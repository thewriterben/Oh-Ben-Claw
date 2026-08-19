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
    until 2026-08-18: ten files against a hundred and ninety-two, so it was
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
# The fix moved to scripts/console.py on 2026-08-15, after a third script hit
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

DECL_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?"
    r"(?:struct|enum|trait|fn|const|static|type|union|mod)\s+"
)
IMPL_RE = re.compile(r"^\s*impl\b")
NOISE_RE = re.compile(r"^\s*(?://|#\[|#!\[)")


def own_file_uses(text: str, name: str) -> int:
    """Occurrences of `name` in its own file that are neither its declaration
    nor a comment nor an attribute.

    The discriminator this whole section turns on. `BrowserNavigateTool` is
    referenced nowhere outside `browser.rs` and the file reports it unused --
    but `all_browser_tools_with_reach`, three hundred lines down in the same
    file, constructs it, and *that* function is called from
    `obc-tools/src/lib.rs`. The type is reached, through a factory, and
    reporting it as unused reads as "the browser suite is dead".

    `A2AClient` looks identical to a scan that stops at the file boundary and is
    not: its only two mentions are `pub struct A2AClient` and `impl A2AClient`,
    so nothing constructs it anywhere, including at home.

    Skipping declarations is what separates those. A struct with an impl block
    scores two mentions and zero uses; one `Name::new()` in a factory scores
    one use. Comments and attributes are dropped for the same reason -- a doc
    example that names the type is not a caller.

    `impl Trait for Type` is counted as a use of *Trait* and not of *Type*, and
    getting that backwards produced this function's first false positive.
    `NodeSelfTest` is a trait with one implementor in its own file and a doc
    that correctly says it is wired in `tests/offgrid_fleet_loop.rs`; skipping
    the whole `impl` line reported it as constructed nowhere and claimed as
    shipped. Implementing a trait is using it. Being on the right-hand side of
    `for` is part of your own definition.

    Known limitation, not fixed: the corpus has `use` lines stripped, so a trait
    that is imported to bring its methods into scope and never named again in
    the body is invisible from outside. That is why the check below only reports
    an item when the docs also name it -- a bare list of these would be mostly
    traits used exactly that way.
    """
    pat = re.compile(rf"\b{re.escape(name)}\b")
    n = 0
    for line in text.splitlines():
        if NOISE_RE.match(line):
            continue
        head = line.split("{", 1)[0]
        if IMPL_RE.match(line):
            trait_part, _, type_part = head.partition(" for ")
            if type_part and pat.search(trait_part):
                n += 1          # `impl Name for X` — a use of the trait
                continue
            if pat.search(head):
                continue        # inherent `impl Name` — part of its definition
        elif DECL_RE.match(line) and pat.search(head) and re.search(
            rf"(?:struct|enum|trait|fn|const|static|type|union|mod)"
            rf"(?:<[^>]*>)?\s+{re.escape(name)}\b", head
        ):
            continue            # the declaration itself
        n += len(pat.findall(line))
    return n


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

    # Of the items nothing outside references, which are constructed at home?
    unref = sorted(set(usable) - set(hits))
    internal = sorted(n for n in unref if own_file_uses(text, n) > 0)
    nowhere = sorted(n for n in unref if n not in set(internal))

    loc = len(text.splitlines())
    rows.append({
        "file": f.relative_to(ROOT).as_posix(),
        "loc": loc,
        "pub": len(usable),
        "used": len(hits),
        "prod": prod,
        "test": test,
        "unref": unref,
        "internal": internal,
        "nowhere": nowhere,
        # Every public name this file declares, kept so the ITEM_ALIASES guard
        # can tell "renamed or deleted" from "still here and simply in use".
        "pub_names": sorted(usable),
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
    # Added 2026-08-19. 22 rows of `| Feature | ✗ | ✅ |` comparing this project
    # to ZeroClaw — one of the densest shipped-claim surfaces in the repository,
    # and one nothing had ever checked. A tick in a comparison table is a claim
    # in its strongest form: it is read as settled, it is read fast, and it is
    # the format a reader trusts *because* it does not argue.
    #
    # It is not a plan, which is the test the rest of docs/ fails: ENDGAME is a
    # running record, BENCH and WALKTHROUGH are procedures, PHASE and V2 are
    # plans. This one asserts, in the present tense, what the system has.
    "docs/architecture/ARCHITECTURE.md",
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
# A marker that resolves to nothing discloses nothing, and this survey has no
# way to tell that from a component nobody disclosed: both produce a "—" in the
# column below. On 2026-08-19 `crates/obc-movement/src/feedback.rs` was reported
# undisclosed while ROADMAP.md disclosed it in full, three screens up.
#
# `scripts/check_tree.py` gates this in CI. The refusal here is not redundant
# with that: it is the difference between a gate somebody can skip and a
# measurement that will not lie about itself when run by hand.
_stale_markers = sorted(p for p in DISCLOSED_BY if not (ROOT / p).exists())
if _stale_markers:
    print(f"!! {len(_stale_markers)} `unwired:` marker(s) name a file that does "
          f"not exist:", file=sys.stderr)
    for p in _stale_markers:
        print(f"!!   {p}  (in {DISCLOSED_BY[p]})", file=sys.stderr)
    print("!! a marker that resolves to nothing discloses nothing — the column "
          "below would\n!! report the component as undisclosed and be wrong. "
          "Fix the path.", file=sys.stderr)
    raise SystemExit(2)
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
    # Repointed 2026-08-18, having been unmatched since each crate was extracted:
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


ITEM_ALIASES = {
    # "When a document claims this feature in prose, it is claiming these items."
    #
    # DOC_ALIASES does this for files. This is the same idea one level down, and
    # it exists because adding `docs/architecture/ARCHITECTURE.md` to DOC_NAMES
    # on 2026-08-19 changed the output by exactly one number -- the count of
    # documents checked -- and caught nothing.
    #
    # The reason is worth writing down, because it is this script's own
    # recurring finding aimed at itself. `item_claims` matches `` `name` `` or
    # `**name**`, deliberately, so that prose using a common word cannot trip
    # the column people act on. ARCHITECTURE.md's 22 shipped claims are a
    # comparison table written entirely in prose -- `| Reflexion /
    # Plan-and-Execute | x | ok |` names no symbol. So a document was added to
    # the claim set that could not match anything in it: a claim-check wearing a
    # claim-check's clothes, which is the failure DOC_ALIASES' own guard was
    # written for, one level up.
    #
    # Keys are public item names as the scan sees them. Values are exact strings
    # to look for in the claim documents, and they should be specific enough
    # that no other row could produce them.
    # `"Reflexion loop"` was here too and the guard below rejected it on the
    # first run: that phrasing exists only in CHANGELOG.md, which is not a claim
    # document. An alias is allowed to be specific; it is not allowed to be
    # aspirational.
    "reflexion_loop": ("Reflexion / Plan-and-Execute",),
    "create_plan": ("Reflexion / Plan-and-Execute", "Plan-and-Execute"),
    "synthesize_results": ("Reflexion / Plan-and-Execute", "Plan-and-Execute"),
}


def item_claims(name: str) -> list[str]:
    """Where the docs name this public item as a thing that exists.

    The file-level check above only asks about files where *nothing* is
    referenced. `A2AClient` is constructed nowhere in the workspace and
    ROADMAP.md ticks it `- [x]`, but it lives in `obc-a2a/src/lib.rs` alongside
    twenty live items, so the file is not flagged and the claim was never
    checked. README.md already says, in a blockquote, that it is "constructed
    **nowhere**" — two documents disagreeing, with nothing comparing them.

    Backticked or bolded only. A bare word would match prose that happens to use
    the name, and this is the column people act on.
    """
    needles = (f"`{name}`", f"**{name}**") + ITEM_ALIASES.get(name, ())
    return [doc for doc, text in DOCS.items()
            if any(n in text for n in needles)]


dead = [r for r in rows if r["used"] == 0]
for r in dead:
    r["docs"] = doc_claims(r["file"])
test_only = [r for r in rows if r["used"] and r["prod"] == 0 and r["test"]]

# Item-level overclaims: constructed nowhere at all, and named in a document
# that says what ships. Only for files that are otherwise alive — a wholly
# unwired file is already reported above, and reporting it twice would double
# the loudest number in the output.
item_over = []
for r in rows:
    if r["used"] == 0:
        continue
    for name in r["nowhere"]:
        claims = item_claims(name)
        if claims:
            item_over.append((name, r["file"], claims))
item_over.sort(key=lambda t: t[0])

# The guard DOC_ALIASES has, for this table. Two ways an entry can be dead
# weight, and both look like a clean result:
#   * the key is not a public item any longer -- renamed, or the file went
#   * the needle is not in any claim document -- the row was reworded, or the
#     document left DOC_NAMES
# Either way the alias stops binding a claim to code and nothing says so, which
# is precisely the condition this table was added to fix.
_all_items = {name for r in rows for name in r["pub_names"]}
_dead_alias_items = [k for k in ITEM_ALIASES if k not in _all_items]
_dead_alias_needles = [
    (k, n) for k, ns in ITEM_ALIASES.items() for n in ns
    if not any(n in text for text in DOCS.values())
]
if _dead_alias_items or _dead_alias_needles:
    print(f"!! {len(ITEM_ALIASES)} item alias(es) declared, and some bind nothing:",
          file=sys.stderr)
    for k in _dead_alias_items:
        print(f"!!   {k}: not a public item in this scan", file=sys.stderr)
    for k, n in _dead_alias_needles:
        print(f"!!   {k}: {n!r} appears in no claim document", file=sys.stderr)
    print("!! an alias that binds nothing is an unchecked claim wearing a checked "
          "one's clothes.\n!! Repoint it, or delete it.", file=sys.stderr)

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

# ── Disclosed here, ticked as shipped there ─────────────────────────────────
# A `<!-- unwired: -->` marker suppresses the overclaim flag for a file, and
# should: the marker exists so that being honest about a gap does not make this
# tool shout louder. But it says one thing -- "this component is parked" -- and
# it was silently doing a second: excusing *every other document* from ever
# ticking the same component as shipped.
#
# Found on 2026-08-19, one commit after ARCHITECTURE.md joined the claim set.
# ROADMAP.md discloses `crates/obc-memory/src/heartbeat.rs` as unwired with a
# stated condition to wire it. ARCHITECTURE.md row 206 reads
# `| Proactive tasks | x | ok HEARTBEAT.md |`. Both are in this repository, both
# are in the claim set, and the marker in one of them made the tick in the other
# unreportable.
#
# So the marker's scope is the document that carries it. Another document is
# free to disagree, and when it does, that is a contradiction a reader can walk
# straight into -- and worse than a plain overclaim, because the repository
# demonstrably knows better somewhere else.
contradictions = []
for r in dead:
    if r["file"] not in DISCLOSED:
        continue
    elsewhere = [d for d in r["docs"] if d != DISCLOSED_BY[r["file"]]]
    if elsewhere:
        contradictions.append((r["file"], DISCLOSED_BY[r["file"]], elsewhere))
contradictions.sort()

print("\n── Disclosed as unwired in one document, presented as shipped in another ──")
if _missing:
    print("  doc cross-reference UNAVAILABLE — see the warning above.")
elif contradictions:
    print(f"{'file':<44}{'disclosed in':<18}also claimed in")
    for file, by, elsewhere in contradictions:
        print(f"{file:<44}{by:<18}{', '.join(elsewhere)}")
    print(f"\n{len(contradictions)} contradiction(s). The disclosure is doing its "
          f"job; the other\ndocument has not been told. Fix the tick, not the "
          f"marker — a second marker\nwould only teach both pages to disagree "
          f"quietly.")
else:
    print("  none")

print("\n── Referenced only from tests ──")
if test_only:
    for r in sorted(test_only, key=lambda r: -r["loc"]):
        print(f"  {r['file']:<44}{r['loc']:>6} LOC   {r['test']} test ref(s)")
    print("  (legitimate for a protocol surface pinned by a conformance suite;")
    print("   a2a is the known intentional case. Anything else, check.)")
else:
    print("  none")

print("\n── Public items constructed nowhere, and claimed as shipped ──")
if _missing:
    print("  doc cross-reference UNAVAILABLE — see the warning above.")
elif item_over:
    print(f"{'item':<28}{'declared in':<46}claimed in")
    for name, file, claims in item_over:
        print(f"{name:<28}{file:<46}{', '.join(claims)}")
    print(f"\n{len(item_over)} item(s). Each is a public name that nothing in the "
          "workspace\nconstructs — not even its own file — inside a file that is "
          "otherwise live, so\nthe file-level list above cannot see it. A document "
          "that says what ships names\nit anyway.")
else:
    print("  none")

partial = [r for r in rows if r["used"] and r["unref"]]
nowhere_total = sum(len(r["nowhere"]) for r in partial)
internal_total = sum(len(r["internal"]) for r in partial)
print(f"\n── Files with some unreferenced public items ── ({len(partial)} files)")
print(f"   {nowhere_total} constructed nowhere; {internal_total} reached only from "
      f"inside their own file")
print("   (the second kind is usually a factory — `all_browser_tools` builds the "
      "seven browser\n    tools three hundred lines below where they are declared, "
      "and is itself called from\n    obc-tools/src/lib.rs. Counting those as "
      "unused reads as 'the suite is dead'.)")
for r in sorted(partial, key=lambda r: -len(r["nowhere"]))[: (len(partial) if show_all else 12)]:
    if not r["nowhere"] and not show_all:
        continue
    shown = r["nowhere"] or r["internal"]
    kind = "nowhere" if r["nowhere"] else "in-file only"
    print(f"  {r['file']:<44}{len(shown):>3} of {r['pub']:>3} {kind}: "
          f"{', '.join(shown[:5])}{' …' if len(shown) > 5 else ''}")
if not show_all and len(partial) > 12:
    print(f"  … {len(partial) - 12} more files (--all)")

if impl_only:
    print(f"\n── No countable public API (impl-only / generic names) ── ({len(impl_only)} files)")
    for f, items in impl_only[: (len(impl_only) if show_all else 8)]:
        print(f"  {f.relative_to(ROOT).as_posix():<44}{', '.join(items[:4])}")
    if not show_all and len(impl_only) > 8:
        print(f"  … {len(impl_only) - 8} more (--all)")

print("\nName-based, not a compiler. A flagged file is worth reading; a cleared")
print("file is not proven live. See the module note at the top of this script.")
