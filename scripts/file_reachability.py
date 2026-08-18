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
field on `SecurityContext` and it is constructed at startup. What never happens is
anyone *asking* it anything. `pair_node` has zero callers, `is_trusted` has zero,
the `SecurityContext.pairing` field is never read, and `security.require_pairing`
is consulted only by config validation — set it and it gates nothing.

(Both this file and the type moved: `crates/obc-safety/src/lib.rs`. The name
`SecurityManager` written here for weeks belongs to nothing in the tree and
never did — a plausible-sounding owner is worse than none, because it survives
a reader checking whether the claim is still true.)

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
CRATES = ROOT / "crates"


def workspace_files() -> list[Path]:
    """Every Rust source in the workspace, binary and crates alike.

    Third survey to need this, after `inert_components.py` and
    `curation_survey.py`, and the last of the family. It scanned `src/` alone
    until 2026-08-14: ten files against a hundred and ninety-two, so it was
    reporting on 5% of the tree and saying "4 files with public API" as though
    that were the codebase.

    The case that made it concrete came from the other direction. `StoredMessage`
    sits in obc-memory with exactly the fields a user interface needs, declared
    once and referenced nowhere in the workspace — an unreferenced public item,
    which is precisely the second list this script prints. It went unfound for as
    long as the crate it lives in was out of scope, and turned up instead by
    someone trying to compile the GUI.

    Widening moves the subjects and the evidence together, for the same reason
    as its siblings: a public item used from another crate is used.
    """
    out = sorted(SRC.rglob("*.rs"))
    if CRATES.is_dir():
        out += sorted(CRATES.glob("*/src/**/*.rs"))
    return out

# The section rules below are drawn with box characters, and a Windows console
# defaults to cp1252, which cannot encode them. Printing one raised
# UnicodeEncodeError and killed the run *after* the survey had done its work
# and printed its counts -- a crash that looks like a broken tool and is
# actually a broken terminal.
#
# The fix moved to scripts/console.py on 2026-08-14, after a third script hit
# it. Two copies were a duplication; three would have been this repository's
# own finding about instruments that record a lesson without enforcing it.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402

use_utf8_stdout()

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


files = workspace_files()
scan_roots = [SRC, CRATES] + [ROOT / d for d in ("tests", "examples", "benches", "gui", "planner-wasm")]
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
#
# Which documents count, and why this is a list rather than `docs/*.md`.
#
# It was README.md and ROADMAP.md until 2026-08-14, and that is how a real
# overclaim hid. `src/deployment/saga.rs` was flagged here as referenced
# nowhere, and reported as claimed by nothing -- while
# `docs/ACCELERAPP-CROSS-POLLINATION.md` listed it under "Also delivered". Two
# documents is not the same set as "the documents that say what ships", and the
# difference is invisible in the output: an unclaimed file and an unchecked
# claim print identically.
#
# So the set is the documents that present the software as *shipped*. It is
# named rather than globbed because most of docs/ is the opposite kind: ENDGAME
# is a running record, the BENCH and WALKTHROUGH pages are procedures, the
# PHASE and V2 pages are plans. A plan describing something unbuilt is doing its
# job; counting it as an overclaim would train someone to ignore this column.
DOC_NAMES = (
    "README.md",
    "ROADMAP.md",
    "docs/ACCELERAPP-CROSS-POLLINATION.md",   # "delivered" ledger vs a sibling project
    "docs/SUBSYSTEM-SUITES-STATUS.md",        # per-subsystem shipped/not table
    "docs/SAFETY-CASE.md",                    # asserts what enforcement exists
    "docs/ECOSYSTEM-INTEGRATION.md",          # asserts what integrates today
    "docs/EMBODIED-ARCHITECTURE.md",          # asserts what the running system does
)
DOCS = {name: strip_audit(read(ROOT / name)) for name in DOC_NAMES}

# A file a document deliberately discloses as unwired is not an overclaim.
#
# This is `strip_audit`'s problem one level up. That function exists because
# embedding the generated tables into ROADMAP.md made every unwired file "named
# in the docs", and a measurement that counts the report about itself converges
# on a lie. The same thing happens to a *correction*: the paragraph written to
# say "this is a mechanism with no caller" names the file, so the next run reads
# it as a claim that the file ships. Being honest about a gap would make the
# tool shout louder.
#
# So a document can say so, once, in a form nobody writes by accident:
#
#     <!-- unwired: src/deployment/saga.rs -->
#
# The file is still listed, still counted as unwired, and still worth reading.
# It is not counted as an overclaim, and the document that disclosed it is
# named instead. Silence is still not an option -- this is the opposite of
# silence, and it costs an author one deliberate line.
DISCLOSED = {
    m.group(1).strip()
    for text in DOCS.values()
    for m in re.finditer(r"<!--\s*unwired:\s*(\S+?)\s*-->", text)
}
DISCLOSED_BY = {
    m.group(1).strip(): name
    for name, text in DOCS.items()
    for m in re.finditer(r"<!--\s*unwired:\s*(\S+?)\s*-->", text)
}
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
    # This table says "when README says HEARTBEAT it means this file".
    #
    # On 2026-08-14 not one of the seven keys resolved. The scan walked `src/`
    # and extraction had moved or deleted every one of them, so every alias
    # silently stopped matching and the overclaim column went quiet for the best
    # possible reason and the worst possible presentation. The guard below was
    # added that day and shouted about it; the scan now covers `crates/`, so the
    # four that merely moved are repointed at where they went.
    #
    # Repointed 2026-08-14, having been unmatched since each crate was extracted:
    "crates/obc-memory/src/heartbeat.rs": ("HEARTBEAT",),
    "crates/obc-memory/src/journal.rs": ("journal",),
    "crates/obc-safety/src/pairing.rs": ("pairing",),
    "crates/obc-audio/src/lib.rs": ("audio pipeline", "Audio pipeline", "audio_pipeline"),
}

# Deleted by gate 3, kept as a record of what the docs used to claim.
#
# Separate from the table above because the guard's question is different for
# these. A live alias that does not resolve is a broken input; a retired one
# that does not resolve is the point. Keeping both in one dict meant the guard
# could only be right about one of them, which is how it ended up reporting
# "7 of 7" — three of those seven were working as intended.
RETIRED_ALIASES = {
    "src/peripherals/fusion.rs": ("Sensor fusion", "sensor fusion"),
    "src/mission/bt.rs": ("behavior tree", "Behavior Tree", "BT engine"),
    "src/runtime/mod.rs": ("Sandbox", "sandbox"),
}

# The other half of the guard above DOCS. A missing input must not read as a
# clean result, and an alias table that matches nothing is a missing input
# wearing a full table's clothes.
_stale_aliases = [k for k in DOC_ALIASES if not (ROOT / k).exists()]
if _stale_aliases:
    print(f"!! {len(_stale_aliases)} of {len(DOC_ALIASES)} doc aliases name files "
          f"that do not exist:", file=sys.stderr)
    for k in _stale_aliases:
        print(f"!!   {k}", file=sys.stderr)
    print("!! their doc terms are not being checked against anything. If the file "
          "moved, repoint the\n"
          "!! key; if it was deleted on purpose, move the entry to "
          "RETIRED_ALIASES.", file=sys.stderr)

# The inverse guard: a retired alias whose file came back is a live one filed
# under the wrong heading, and would be silently excluded from the scan.
_revived = [k for k in RETIRED_ALIASES if (ROOT / k).exists()]
if _revived:
    print(f"!! {len(_revived)} retired doc alias(es) name files that exist again: "
          f"{', '.join(_revived)}", file=sys.stderr)
    print("!! move them back to DOC_ALIASES so their doc terms are checked.",
          file=sys.stderr)


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
        if r["file"] in DISCLOSED:
            mark = f"disclosed unwired in {DISCLOSED_BY[r['file']]}"
            flag = ""
        else:
            mark = ", ".join(r["docs"]) if r["docs"] else "—"
            flag = "  <-- OVERCLAIM" if r["docs"] else ""
        print(f"{r['file']:<40}{r['loc']:>6}{r['pub']:>5}  {mark}{flag}")
    claimed = [r for r in dead if r["docs"] and r["file"] not in DISCLOSED]
    print(f"\n{len(dead)} unwired file(s), {sum(r['loc'] for r in dead):,} LOC.")
    if _missing:
        print("doc cross-reference UNAVAILABLE — see the warning above. Not a clean bill.")
    else:
        print(f"of those, {len(claimed)} are presented as shipped in "
              f"{len(DOCS)} shipped-claim document(s) "
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
