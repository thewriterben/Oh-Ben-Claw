"""Curation input: which modules are load-bearing, which are islands.

Not a dead-code detector. It answers one question — is this module referenced
from anywhere outside its own directory? — which is the cheapest honest proxy
for "would removing it break the build". A module with zero external references
is a candidate for the first public cut; it is not proof of deadness, and
anything with a `main.rs` reference is load-bearing regardless.

Two corrections, 2026-07-29
---------------------------
The previous version reported four islands — `gateway`, `tunnel`, `runtime`,
`bin` — and three of those were wrong.

1. **Grouped imports were invisible.** The pattern was

       \\b(?:crate|oh_ben_claw)::{mod}\\b | ^\\s*use\\s+{mod}::

   which cannot match `use oh_ben_claw::{config, gateway, …}` — the module name
   is inside a brace group, not after a `::`. That is how most of this crate is
   consumed. `gateway` (2,313 LOC, binds the whole HTTP API) and `tunnel` were
   both reported dead on the strength of it. A survey that reports the API
   gateway as removable is worse than no survey, because the one time it is
   believed is the time it does damage. Use-trees are now brace-matched and
   expanded, including nested ones (`{a, b::{c, d}}`).

2. **`src/bin/` is not a module.** It is Cargo's auto-discovered binary
   directory and is not declared in `src/lib.rs`, so "no external references" is
   its normal state. The module list now comes from `pub mod` declarations in
   `src/lib.rs` rather than from directory listing, which excludes it by
   construction and also means this script can never disagree with the crate
   about what a module is.

After both fixes the only genuine island was `runtime` (417 LOC) — see the
ROADMAP note against "Sandboxed tool execution". `runtime` has since been cut,
and the survey now reports no islands at all.

The import parser moved to `scripts/rust_imports.py` on 2026-08-01, when
`extractability.py` needed the same brace matcher. Two copies of it would have
been this script's own headline finding turned on itself: the 2026-07-29
correction took two wrong answers with it, and a second copy would have kept one
of them alive. Output is byte-identical across that refactor, which is how the
move was checked.

Consumers scanned: src/, tests/, examples/, benches/, gui/, planner-wasm/.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rust_imports import declared_modules, direct_ref, use_tree_heads  # noqa: E402

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
SRC = ROOT / "src"
LIB = SRC / "lib.rs"

# The crate's own view of what a module is. Directory listing would also pick up
# src/bin/ (a Cargo convention, not a module) and any scratch directory.
if not LIB.exists():
    sys.exit(f"no {LIB} — run from the repo root or pass the root as argv[1]")
declared, _external = declared_modules(LIB)
mods = sorted(m for m in declared if (SRC / m).is_dir())
file_mods = sorted(m for m in declared if not (SRC / m).is_dir())

# Scan tests/ and examples/ too. An earlier version looked only at src/ and reported
# `a2a` as unreferenced; tests/evals.rs pins its wire shape as a release gate. An
# integration test is a consumer, and the one most likely to be the ONLY consumer of a
# protocol surface that has not shipped yet — precisely the case this is meant to find.
files = list(SRC.rglob("*.rs"))
for extra in ("tests", "examples", "benches", "gui", "planner-wasm"):
    d = ROOT / extra
    if d.is_dir():
        files += list(d.rglob("*.rs"))

rows = []
# Pre-compute per-file evidence once, instead of re-reading every file per module.
per_file: list[tuple[Path, set[str], str]] = []
for f in files:
    text = f.read_text(encoding="utf-8", errors="replace")
    per_file.append((f, use_tree_heads(text), text))

for m in mods:
    own = SRC / m
    direct = direct_ref(m)
    loc = sum(sum(1 for _ in f.open(encoding="utf-8", errors="replace"))
              for f in own.rglob("*.rs"))
    ext, in_main = 0, False
    for f, heads, text in per_file:
        try:
            f.relative_to(own)
            continue
        except ValueError:
            pass
        if m in heads or direct.search(text):
            ext += 1
            if f.name == "main.rs":
                in_main = True
    rows.append((m, loc, ext, in_main))

rows.sort(key=lambda r: (r[2], -r[1]))
print(f"{'module':<16}{'loc':>7}{'ext refs':>10}  main")
print("-" * 44)
for m, loc, ext, in_main in rows:
    print(f"{m:<16}{loc:>7}{ext:>10}  {'yes' if in_main else ''}")

islands = [r for r in rows if r[2] == 0]
print(f"\nzero external references: {len(islands)} module(s), {sum(r[1] for r in islands)} LOC")
print("  " + (", ".join(r[0] for r in islands) if islands else "none"))
print(f"\ntotal: {len(rows)} directory modules, {sum(r[1] for r in rows)} LOC")
if file_mods:
    print(f"single-file modules (not surveyed): {', '.join(file_mods)}")
print("\nReminder: zero external references means 'removing it would not break the")
print("build'. That is where the judgement starts, not where it ends.")
