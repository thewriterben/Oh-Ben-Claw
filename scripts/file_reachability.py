"""File-level reachability: which files export things nothing uses.

Why this exists
---------------
`scripts/curation_survey.py` and `scripts/subsystem_ledger.py` both work at
module-directory granularity. Neither can see a dead *file* inside a live module,
and that is precisely where this codebase's two worst overclaims lived:

  memory/personality.rs   SOUL.md / USER.md. Implemented, documented in ROADMAP
                          and README, never called. Found by hand, 2026-07-28.
                          This script finds files of that shape.

So: the same question the module survey asks, one level down. On the tree as of
2026-07-30 it finds 17 unwired files, 14 of which README or ROADMAP presents as
shipped.

The failure mode it does NOT catch
----------------------------------
`security/pairing.rs` is the reason this script was started, and it does not
appear in its output — correctly. `NodePairingManager` *is* referenced: it is a
field on `SecurityManager` and it is constructed at startup. What never happens is
anyone *asking* it anything. `pair_node` has zero callers, `is_trusted` has zero,
the `SecurityManager.pairing` field is never read, and `security.require_pairing`
is consulted only by config validation — set it and it gates nothing.

That is a third category, distinct from a dead file: **built, wired, never
invoked.** Reachability cannot see it, because the type genuinely is reachable.
Catching it needs call-graph analysis at method granularity, which this script
does not attempt. Until something does, that class is found by reading code.

The honest summary of coverage:
  dead file, nothing references it          -> caught here
  live file, some items unused              -> reported as a secondary list
  live type, constructed, never interrogated -> NOT CAUGHT. See pairing.rs.

    python scripts/file_reachability.py           # files with no external use
    python scripts/file_reachability.py --all     # every file, with counts

How it decides
--------------
For each file, collect the names it declares `pub`. Then count references to
those names elsewhere in the crate, and *ignore import lines* — a `pub use
pairing::NodePairingManager` in `security/mod.rs` is a re-export, not a consumer.
Counting it would let any module launder its own dead files into liveness by
re-exporting them, which is the exact mistake that made `pairing.rs` look alive.

Consumers are split into production and test, because "used only by tests" is a
real and sometimes correct state — `a2a` is deliberately pinned by
`tests/evals.rs` alone — but it is a different state from "used".

Limits, stated because a survey that overstates itself is worse than none:
  * Name-based, not a compiler. A short or common item name (`new`, `Config`,
    `State`) will match unrelated code and read as live. False *negatives* are
    the failure mode; a file this flags is worth looking at, a file it clears is
    not proven live.
  * Trait-object dispatch and macro-generated calls are invisible.
  * A file whose only pub item is a trait `impl` for a type declared elsewhere
    has nothing to count, and is reported separately rather than as dead.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

# Names too generic for name-based matching to say anything. Matching these
# produces confident nonsense in both directions.
TOO_GENERIC = {
    "new", "default", "build", "run", "start", "stop", "get", "set", "name",
    "value", "State", "Config", "Error", "Result", "Kind", "Status", "Entry",
    "Item", "Event", "Message", "Response", "Request", "Handle", "Manager",
    "Builder", "Options", "Params", "Data", "Info", "Meta", "Id", "Key",
}

PUB_ITEM = re.compile(
    r"^\s*pub(?:\([^)]*\))?\s+"
    r"(?:async\s+|const\s+|unsafe\s+|extern\s+(?:\"[^\"]*\"\s+)?)*"
    r"(?:fn|struct|enum|trait|type|const|static|union)\s+"
    r"([A-Za-z_][A-Za-z0-9_]*)",
    re.M,
)
# `use` / `pub use` lines, possibly spanning braces — never count as consumption.
IMPORT_LINE = re.compile(r"^\s*(?:pub\s+)?use\s+", re.M)


def read(p: Path) -> str:
    try:
        return p.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def strip_imports(text: str) -> str:
    """Blank out use/pub-use statements, including multi-line brace groups."""
    out, i = [], 0
    lines = text.splitlines(keepends=True)
    while i < len(lines):
        if IMPORT_LINE.match(lines[i]):
            depth = lines[i].count("{") - lines[i].count("}")
            out.append("\n")
            i += 1
            while i < len(lines) and (depth > 0 or ";" not in lines[i - 1]):
                depth += lines[i].count("{") - lines[i].count("}")
                out.append("\n")
                done = depth <= 0 and ";" in lines[i]
                i += 1
                if done:
                    break
        else:
            out.append(lines[i])
            i += 1
    return "".join(out)


def module_level_items(text: str) -> set[str]:
    """Public items declared at module level — not methods inside `impl` blocks.

    This distinction is the whole accuracy of the script. A first version matched
    `pub fn` anywhere, which pulled in method names from inside `impl` blocks:
    `verify`, `status`, `generate`, `revoke`. Those are common words, they match
    across the tree, and `security/pairing.rs` — the file this script was written
    to catch — read as comfortably alive because of them. Types and free
    functions are the reliable signal; methods are only reachable through a type
    that has to be named somewhere anyway.

    Brace counting ignores braces inside strings and comments. That can drift the
    depth on unusual code, so it is a heuristic, in a script that already says it
    is a heuristic.
    """
    items: set[str] = set()
    depth = 0
    for line in text.splitlines():
        if depth == 0:
            m = PUB_ITEM.match(line)
            if m:
                items.add(m.group(1))
        depth += line.count("{") - line.count("}")
        if depth < 0:
            depth = 0
    return items


def strip_audit(text: str) -> str:
    """Remove fenced blocks and the generated audit section from a doc.

    Without this, these scripts read their own output. Embedding the generated
    tables into ROADMAP.md immediately moved both numbers: the file sweep went
    from "14 of 17 documented as shipped" to "17 of 17", and the ledger's
    "5 modules with no ROADMAP presence" became 0 — because every module and every
    unwired file is now *named in the audit section*, which is not the same as
    being claimed as a working feature. A measurement that counts the report about
    itself converges on a lie, and the direction of the error is reassuring, which
    makes it worse.
    """
    out, fenced = [], False
    in_ledger = False
    for line in text.splitlines():
        if line.startswith("```"):
            fenced = not fenced
            continue
        if line.startswith("## "):
            in_ledger = line.strip() == "## Subsystem ledger"
        if fenced or in_ledger:
            continue
        # A correction is not a claim. Blockquotes are how this repo records "this
        # was advertised and does not run", and `[~]` / `[-]` are the roadmap
        # markers for the same thing. Counting them kept the overclaim total at 9
        # after nine overclaims had just been struck — the tool reporting no
        # progress precisely because the progress was made in prose it was reading.
        stripped = line.lstrip()
        if stripped.startswith(">") or stripped.startswith("- [~]") or stripped.startswith("- [-]"):
            continue
        out.append(line)
    return "\n".join(out)


def is_test_file(p: Path) -> bool:
    return (ROOT / "tests") in p.parents or p.name.endswith("_test.rs")


files = sorted(SRC.rglob("*.rs"))
scan_roots = [SRC] + [ROOT / d for d in ("tests", "examples", "benches", "gui", "planner-wasm")]
corpus: list[tuple[Path, str]] = []
for r in scan_roots:
    if r.is_dir():
        for f in r.rglob("*.rs"):
            corpus.append((f, strip_imports(read(f))))

show_all = "--all" in sys.argv
rows, impl_only = [], []

for f in files:
    text = read(f)
    items = module_level_items(text)
    usable = {i for i in items if i not in TOO_GENERIC and len(i) > 3}

    if not items:
        continue
    if not usable:
        impl_only.append((f, sorted(items)))
        continue

    prod = test = 0
    hits: dict[str, int] = {}
    for name in usable:
        pat = re.compile(rf"\b{re.escape(name)}\b")
        for g, body in corpus:
            if g == f:
                continue
            n = len(pat.findall(body))
            if n:
                hits[name] = hits.get(name, 0) + n
                if is_test_file(g):
                    test += n
                else:
                    prod += n

    loc = len(text.splitlines())
    rows.append({
        "file": f.relative_to(ROOT).as_posix(),
        "loc": loc,
        "pub": len(usable),
        "used": len(hits),
        "prod": prod,
        "test": test,
        "unref": sorted(set(usable) - set(hits)),
    })

# ── Is a flagged file also *claimed* in the docs? ────────────────────────────
# This is the question that matters. An unwired internal helper is untidy; an
# unwired feature that README or ROADMAP presents as shipped is a promise the
# software does not keep, and that is what both known cases were.
DOCS = {name: strip_audit(read(ROOT / name)) for name in ("README.md", "ROADMAP.md")}
# A missing input must not read as a clean result. Both README.md and ROADMAP.md
# were absent the first time this ran — it was pointed at a partial checkout — and
# it reported "0 of 17 are presented as shipped", which is the most reassuring
# possible way to say "I could not check". Empty-because-absent and
# empty-because-clean have to look different.
_missing = [n for n, t in DOCS.items() if not t]
if _missing:
    print(f"!! cannot check doc claims: {', '.join(_missing)} not found under {ROOT}",
          file=sys.stderr)
    print("!! the overclaim column below is meaningless — run from a full checkout",
          file=sys.stderr)
DOC_ALIASES = {
    "src/memory/heartbeat.rs": ("HEARTBEAT",),
    "src/security/pairing.rs": ("pairing",),
    "src/peripherals/fusion.rs": ("Sensor fusion", "sensor fusion"),
    "src/mission/bt.rs": ("behavior tree", "Behavior Tree", "BT engine"),
    "src/memory/journal.rs": ("journal",),
    "src/runtime/mod.rs": ("Sandbox", "sandbox"),
    "src/audio/mod.rs": ("audio pipeline", "Audio pipeline", "audio_pipeline"),
}


def doc_claims(rel: str) -> list[str]:
    """Where the docs mention this file — by path, by stem, or by declared alias."""
    stem = rel.rsplit("/", 1)[-1].removesuffix(".rs")
    needles = [rel, rel.removesuffix("/mod.rs"), f"`{stem}`"]
    needles += list(DOC_ALIASES.get(rel, ()))
    found = []
    for doc, text in DOCS.items():
        for n in needles:
            if n and n in text:
                found.append(doc)
                break
    return found


dead = [r for r in rows if r["used"] == 0]
for r in dead:
    r["docs"] = doc_claims(r["file"])
test_only = [r for r in rows if r["used"] and r["prod"] == 0 and r["test"]]

print(f"{len(rows)} files with public API, {len(files)} files scanned.\n")

print("── No public item referenced anywhere outside the file ──")
if dead:
    print(f"{'file':<40}{'loc':>6}{'pub':>5}  documented as shipped in")
    for r in sorted(dead, key=lambda r: (not r["docs"], -r["loc"])):
        mark = ", ".join(r["docs"]) if r["docs"] else "—"
        flag = "  <-- OVERCLAIM" if r["docs"] else ""
        print(f"{r['file']:<40}{r['loc']:>6}{r['pub']:>5}  {mark}{flag}")
    claimed = [r for r in dead if r["docs"]]
    print(f"\n{len(dead)} unwired file(s), {sum(r['loc'] for r in dead):,} LOC.")
    if _missing:
        print("doc cross-reference UNAVAILABLE — see the warning above. Not a clean bill.")
    else:
        print(f"of those, {len(claimed)} are presented as shipped in README/ROADMAP "
              f"({sum(r['loc'] for r in claimed):,} LOC) — fix the code or fix the claim.")
else:
    print("none")

print("\n── Referenced only from tests ──")
if test_only:
    for r in sorted(test_only, key=lambda r: -r["loc"]):
        print(f"  {r['file']:<44}{r['loc']:>6} LOC   {r['test']} test ref(s)")
    print("  (legitimate for a protocol surface pinned by a conformance suite;")
    print("   a2a is the known intentional case. Anything else, check.)")
else:
    print("  none")

partial = [r for r in rows if r["used"] and r["unref"]]
print(f"\n── Files with some unreferenced public items ── ({len(partial)} files)")
for r in sorted(partial, key=lambda r: -len(r["unref"]))[: (len(partial) if show_all else 12)]:
    print(f"  {r['file']:<44}{len(r['unref']):>3} of {r['pub']:>3} unused: "
          f"{', '.join(r['unref'][:5])}{' …' if len(r['unref']) > 5 else ''}")
if not show_all and len(partial) > 12:
    print(f"  … {len(partial) - 12} more (--all)")

if impl_only:
    print(f"\n── No countable public API (impl-only / generic names) ── ({len(impl_only)} files)")
    for f, items in impl_only[: (len(impl_only) if show_all else 8)]:
        print(f"  {f.relative_to(ROOT).as_posix():<44}{', '.join(items[:4])}")
    if not show_all and len(impl_only) > 8:
        print(f"  … {len(impl_only) - 8} more (--all)")

print("\nName-based, not a compiler. A flagged file is worth reading; a cleared")
print("file is not proven live. See the module note at the top of this script.")
