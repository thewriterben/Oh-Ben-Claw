"""Which module could move to Open Body Control next, and what is holding it.

`curation_survey.py` asks who references a module — the deletion question.
This asks the opposite one, which the migrate-piecewise policy actually runs on:
**what does a module reference?** A module with many dependents is still easy to
extract (obc-memory had twenty-three); a module with many dependencies is not,
however few things use it.

Three crates have moved so far — obc-memory, obc-planner, obc-safety — and each
was picked by hand, reading imports. That worked and does not scale, and it also
hid the shape of the real work: obc-safety was blocked for months by exactly one
edge pointing the wrong way (`RiskClass` living in `tools`), which nobody had
written down because nobody was counting edges. This script counts them.

An outward edge is **blocking** if it points at a module still in this tree, and
**free** if it points at a crate that has already left — a new crate can just
depend on obc-memory. So the extraction queue is: fewest blocking edges first,
and for each candidate the edge list is the actual to-do list.

Corrected before it was ever committed, 2026-08-01
--------------------------------------------------
The first version counted only `use crate::…` declarations and reported **config
as having zero outward edges** — ready to extract, 58 dependents, the obvious
next piece. It has six. `config/mod.rs` types its own struct fields with inline
paths:

    pub rules: Vec<crate::agent::reflex::ReflexRule>,
    pub server: crate::mcp::McpServerConfig,
    pub missions: Vec<crate::mission::Mission>,

No `use` line anywhere, and every one a compile edge. This is the same failure
`curation_survey.py` was corrected for in July — a matcher that misses how the
crate is actually written, returning a confident wrong answer — which is why
that script carries `direct_ref()` and why this one now uses it too. A survey
that nominates the wrong module for a week of extraction work is worse than no
survey.

What this is not
----------------
A name-based proxy, like its two siblings. It reads `use crate::…` trees and
inline `crate::x::…` paths, with line comments stripped so a mention in prose is
not counted as a dependency. It does not resolve `super::`, which inside a
submodule usually stays within the module but at the top of `mod.rs` means the
crate root — the count of those is printed as a caveat rather than guessed at.
It also cannot see whether an edge is one type or four hundred call sites, and
that difference is the whole cost of an extraction. Read the edges.

Usage: python scripts/extractability.py [repo-root]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rust_imports import declared_modules, use_tree_heads  # noqa: E402

# Inline paths: `pub server: crate::mcp::McpServerConfig`. No `use` line records
# these, and in this crate they are the majority of config's dependencies.
INLINE = re.compile(r"\b(?:crate|oh_ben_claw)\s*::\s*([a-z_][a-z0-9_]*)")
LINE_COMMENT = re.compile(r"//.*")

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
SRC = ROOT / "src"
LIB = SRC / "lib.rs"

if not LIB.exists():
    sys.exit(f"no {LIB} — run from the repo root or pass the root as argv[1]")

mods, external = declared_modules(LIB)


def module_files(name: str) -> list[Path]:
    d = SRC / name
    if d.is_dir():
        return sorted(d.rglob("*.rs"))
    f = SRC / f"{name}.rs"
    return [f] if f.exists() else []


def is_shim(name: str) -> bool:
    """A single file whose only content is `pub use obc_*::…`.

    `src/security.rs` is three lines re-exporting obc-safety. It is declared
    `pub mod`, so it looks like a module in this tree; a reference to
    `crate::security` reaches an already-extracted crate and blocks nothing.
    Counting it as blocking would say obc-safety never left.
    """
    files = module_files(name)
    if len(files) != 1 or (SRC / name).is_dir():
        return False
    body = [
        line.strip()
        for line in files[0].read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("//")
    ]
    return bool(body) and all(line.startswith("pub use obc_") for line in body)


shims = {m for m in mods if is_shim(m)}
# Everything a module can point at without being tied to this tree.
free_targets = set(external) | shims
in_tree = [m for m in mods if m not in shims]

rows = []
for m in in_tree:
    files = module_files(m)
    loc = sum(sum(1 for _ in f.open(encoding="utf-8", errors="replace")) for f in files)
    heads: set[str] = set()
    supers = 0
    for f in files:
        raw = f.read_text(encoding="utf-8", errors="replace")
        # Strip line comments first: `/// see crate::agent` is prose, not a
        # dependency, and counting it would block a module on a sentence.
        text = LINE_COMMENT.sub("", raw)
        heads |= use_tree_heads(text)
        heads |= set(INLINE.findall(text))
        supers += text.count("super::")
    # A head is only an edge if it names something lib.rs declares. `use crate::Config`
    # and stray identifiers are not modules.
    edges = {h for h in heads if h in mods or h in external} - {m}
    blocking = sorted(e for e in edges if e not in free_targets)
    free = sorted(e for e in edges if e in free_targets)
    rows.append((m, loc, blocking, free, supers))

rows.sort(key=lambda r: (len(r[2]), r[1]))

print(f"{'module':<16}{'loc':>7}{'blocking':>10}{'free':>6}  edges (free marked *)")
print("-" * 96)
for m, loc, blocking, free, _ in rows:
    shown = ", ".join(blocking + [f"{e}*" for e in free]) or "—"
    print(f"{m:<16}{loc:>7}{len(blocking):>10}{len(free):>6}  {shown}")

ready = [r for r in rows if not r[2]]
print(f"\nzero blocking edges: {len(ready)} module(s), {sum(r[1] for r in ready)} LOC")
print("  " + (", ".join(r[0] for r in ready) if ready else "none"))

if free_targets:
    print(f"\nalready extracted (free to depend on): {', '.join(sorted(free_targets))}")

near = [r for r in rows if len(r[2]) == 1]
if near:
    print("\none edge from extractable — the shape obc-safety was in until its"
          "\nRiskClass edge was turned around:")
    for m, loc, blocking, _free, _s in near:
        print(f"  {m:<14}{loc:>7} LOC   blocked by: {blocking[0]}")

supers_total = sum(r[4] for r in rows)
if supers_total:
    print(f"\nCaveat: {supers_total} `super::` paths are not resolved. Inside a submodule"
          "\nthey usually stay within the module; at the top of mod.rs they mean the"
          "\ncrate root. Unresolved rather than guessed — a wrong edge here would"
          "\nmisreport a module as ready.")
print("\nEdges are a to-do list, not a cost. One edge can be one type or four"
      "\nhundred call sites, and this script cannot tell the difference.")
