"""The tree on the front page must be a tree this repository has.

Why this exists
---------------
README.md draws the source layout as an ASCII tree. It is the first thing a
newcomer reads to find anything, and on 2026-08-19 it described the repository
as it stood before the extraction campaign that is this project's main body of
work: 24 entries under `src/`, of which 2 still existed. `agent/`, `movement/`,
`navigation/`, `spine/`, `tools/` and eighteen more had become `crates/obc-*`
months earlier -- the moves are individually recorded in ROADMAP.md and
DECISIONS.md, and the picture on the front page was never redrawn.

It also listed 6 crates under `crates/` when there were 33, which reads as a
complete list because nothing says otherwise.

A stale tree fails in a specific way: it does not look stale. Every line is
plausible, the paths are the ones the prose talks about, and a reader only finds
out by opening a directory that is not there.

The two rules
-------------
1. Every path the tree draws must exist.

2. If the tree lists any child of `crates/`, it must list all of them. That
   directory is the one the extraction campaign changes weekly and the one whose
   contents are vendored into OBC-Prime, so a partial list there is the one that
   misleads most. Other parents may be a curated selection -- `docs/` names the
   four documents worth opening first, deliberately -- and rule 1 still holds
   them to existing.

3. Every path inside an `<!-- unwired: PATH -->` marker must exist.

   Different prose, same failure, and a worse one. `file_reachability.py` treats
   that marker as the one way a document may disclose a component as parked
   without the survey reading the disclosure itself as a claim that the
   component ships. So the marker is not decoration -- it is the input to a
   measurement.

   On 2026-08-19 `crates/obc-movement/src/feedback.rs` was reported as unwired
   *and undisclosed* while ROADMAP.md carried a full paragraph disclosing it,
   because the paragraph said `movement/feedback.rs` and no marker had been
   added at all. A stale path in a description misleads a reader. A stale path
   in a marker misleads the instrument, and the instrument is what gets
   believed.

4. Every backticked repository path in a *present-tense* document must exist.

   Rules 1-3 check the tree and the markers -- the two places a path is
   structural. This one checks the other several hundred, where a path is
   written inline in a sentence, and it is the larger surface by an order of
   magnitude.

   It was measured on 2026-08-19 and not built, which is a specific kind of
   mistake worth naming: a count in a conversation is not an instrument, and the
   things it counted go on rotting. `docs/SUBSYSTEM-SUITES-STATUS.md` and
   `docs/playbooks/safing-escalations.md` both pointed at `src/agent/safing.rs`
   -- a path dead since `obc-agent` was extracted on 2026-08-14 -- and were
   found on 2026-08-20 only because the file moved again and someone grepped.

   `docs/SAFETY-CASE.md` is the sharpest case. Its rows are `| control |
   evidence | argument |`, and the evidence column is a file path. Seven of its
   nine safety controls cited a file that has not existed since the security
   modules became `obc-safety`. The argument each row makes is true; the
   evidence it offers cannot be opened. That is worse than an ordinary stale
   link, because a safety case is a document whose entire function is to be
   checkable by someone who does not trust it.

   *Records are not claims.* A line inside a blockquote, or in a paragraph
   carrying a date, is reporting what was once true -- README.md's own note that
   the tree "listed `src/security/limits.rs` ... until 2026-08-02" is a correct
   sentence containing a dead path, and rewriting it would falsify the record.
   This is `strip_audit`'s principle in the fourth place it has come up: a
   measurement that counts the report about a past error as a present claim
   converges on a lie. Eight of the thirty-one paths this rule first found were
   records, and skipping them is not leniency -- counting them would be wrong.

   ROADMAP.md and CHANGELOG.md are excluded entirely for the same reason, not
   as an exception: they are append-only ledgers, dated end to end. 137 of their
   paths do not resolve and every one of them is correct.

What this cannot check
----------------------
That a comment beside a path is true. `movement/ # Track 0-bounded actuation +
closed-loop feedback` pointed at a real directory and described a module
ROADMAP.md lists as deliberately unwired; both statements were in the same
repository, and only a reader who held them side by side would notice. Paths are
checkable and prose is not, which is the reason to keep the prose short.

Nor that a marker exists where one is warranted -- that is
`file_reachability.py`'s job, and it reports a file with no disclosure in its
own output. This checks the markers that are here, not the ones that are not.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402

use_utf8_stdout()

ROOT = Path(__file__).resolve().parent.parent
README = ROOT / "README.md"

# The tree block opens with the repository name on its own line inside a fence.
TREE_HEADER = "Oh-Ben-Claw/"

# Tree-drawing glyphs, and the entry token that follows them.
GLYPHS = "│├└─|`- "
ENTRY_RE = re.compile(r"([A-Za-z0-9_.@-]+(?:\.[A-Za-z0-9]+)?/?)")

# Directories whose full contents rule 2 checks, mapped to names inside them that
# are not crates and so are not expected in the tree.
COMPLETE: dict[str, set[str]] = {"crates": set()}

# The disclosure marker `file_reachability.py` reads. Kept identical to the
# pattern there on purpose: a marker this script accepts and that one ignores
# would be worse than no check.
MARKER_RE = re.compile(r"<!--\s*unwired:\s*(\S+?)\s*-->")

# Where markers are looked for. Everything tracked, not a curated list: a
# disclosure is worth exactly as much wherever it is written, and a marker in a
# document nobody thought to enumerate is the one that rots unnoticed.
MARKER_GLOB = "**/*.md"
MARKER_SKIP = ("target/", "node_modules/", "gui/dist/", ".git/")

# ── Rule 4 ───────────────────────────────────────────────────────────────────

# The documents that present the software as *shipped*. Deliberately the same
# set as `file_reachability.py`'s DOC_NAMES minus the two ledgers, plus the
# vendored playbooks -- those are copied verbatim into OBC-Prime, where a reader
# has no way to check a path against a repository they do not have.
CLAIM_DOCS = (
    "README.md",
    "docs/SUBSYSTEM-SUITES-STATUS.md",
    "docs/SAFETY-CASE.md",
    "docs/ECOSYSTEM-INTEGRATION.md",
    "docs/EMBODIED-ARCHITECTURE.md",
    "docs/architecture/ARCHITECTURE.md",
    "docs/playbooks/safing-escalations.md",
    "docs/playbooks/mesh-node-lost.md",
    "docs/playbooks/vision-analytics.md",
)

# ROADMAP.md, CHANGELOG.md and docs/ACCELERAPP-CROSS-POLLINATION.md are the
# ledgers. Named here rather than merely absent so that adding one to
# CLAIM_DOCS is a decision someone has to reverse on purpose.
LEDGERS = ("ROADMAP.md", "CHANGELOG.md", "docs/ACCELERAPP-CROSS-POLLINATION.md")

# A backticked token that is a path *somewhere else*. The claim is true and the
# file is not ours to have, so requiring it to exist would force the document to
# stop naming it -- which is the opposite of what a comparison document is for.
#
# Guarded below: an entry that no longer appears in any claim document is an
# error, because a suppression nobody can see is how the first stale path got in.
NOT_OURS = {
    # OBC-deployment-generator's own source, named by the document that exists
    # to compare the two repositories side by side.
    "lib/obc-data.ts",
    "lib/firmware-generator.ts",
    "server/routers.ts",
    # Emitted *by* that generator into firmware it writes. `Cargo.toml`,
    # `src/main.rs` and `config.rs` are in the same sentence and happen to
    # resolve here, which is the trap: they would pass for the wrong reason.
    ".cargo/config.toml",
    # The generator's backend, and Accelerapp's codegen trees. Named in the
    # cell *opposite* ours in a three-column comparison table, which is the
    # clearest signal a path is deliberately somebody else's: the row exists
    # to say we have `firmware/obc-esp32-s3/` and they have these.
    "server/",
    "platforms/",
    "rtos/",
}

# What counts as a path rather than a symbol. A backticked token needs a slash
# (`lib.rs` alone is a filename in a sentence, not a location) and then either a
# known extension or a trailing slash.
PATH_EXT = ("rs", "py", "toml", "md", "yml", "yaml", "ino", "json", "ts", "tsx",
            "sh", "lock", "txt", "csv", "html", "css", "svg")
CLAIM_RE = re.compile(r"`([^`\n]+)`")
PATH_RE = re.compile(r"^[A-Za-z0-9_.][A-Za-z0-9_./@-]*(?:\.(?:" +
                     "|".join(PATH_EXT) + r")|/)$")

# A paragraph carrying one of these is reporting, not claiming.
DATE_RE = re.compile(r"20\d\d-\d\d-\d\d")

# ── Rule 5 ───────────────────────────────────────────────────────────────────

# `docs/SAFETY-CASE.md`'s §3 table is `| # | Mechanism | Where | Guarantee |`,
# and on 2026-08-21 the Guarantee column started naming the test that fails when
# each control stops holding. That is the difference between a safety case and a
# list of assertions: a reader who does not trust the author can run the name.
#
# The names rot the way paths rot, and more quietly — a renamed test still
# passes, so nothing goes red, and the citation silently becomes decoration. So
# every test this document cites must exist.
#
# Only the §3 rows, and only tokens with three or more underscores. Test names
# in this repository are sentences (`a_tool_that_understates_its_risk_is_never_
# gated`); three underscores is enough to exclude `risk_class`, `allowed_pins`
# and `value_min`, which are fields, without a list of exceptions to maintain.
SAFETY_CASE = "docs/SAFETY-CASE.md"
CONTROL_ROW_RE = re.compile(r"^\|\s*3\.\d+\s*\|")
TEST_TOKEN_RE = re.compile(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+){3,}$")
FN_RE = re.compile(r"\bfn\s+(\w+)")
RS_GLOB = "**/*.rs"
RS_SKIP = ("target/", "node_modules/")


def cited_tests() -> tuple[list[tuple[int, str]], int]:
    """(line, name) for every test SAFETY-CASE's §3 table cites but does not have."""
    doc = ROOT / SAFETY_CASE
    if not doc.is_file():
        return [(0, f"<{SAFETY_CASE} is missing>")], 0

    defined: set[str] = set()
    for rs in ROOT.glob(RS_GLOB):
        rel = rs.relative_to(ROOT).as_posix()
        if any(rel.startswith(s) or f"/{s}" in f"/{rel}" for s in RS_SKIP):
            continue
        defined.update(FN_RE.findall(rs.read_text(encoding="utf-8", errors="replace")))

    missing, cited = [], 0
    for n, line in enumerate(doc.read_text(encoding="utf-8", errors="replace")
                             .splitlines(), 1):
        if not CONTROL_ROW_RE.match(line):
            continue
        for tok in CLAIM_RE.findall(line):
            tok = tok.strip()
            if not TEST_TOKEN_RE.match(tok):
                continue
            cited += 1
            if tok not in defined:
                missing.append((n, tok))
    return missing, cited



def record_lines(lines: list[str]) -> set[int]:
    """1-based line numbers that record rather than claim.

    Two shapes, both conservative. A blockquote is a quotation -- of an older
    note, an audit, a review -- and a quotation that has been silently corrected
    is no longer a quotation. A paragraph containing a date is dated by its
    author, and a dead path in it is usually the point of the sentence.
    """
    out: set[int] = set()
    for n, line in enumerate(lines, 1):
        if line.lstrip().startswith(">"):
            out.add(n)
    start = 1
    for n, line in enumerate(lines + [""], 1):
        if not line.strip():
            if any(DATE_RE.search(x) for x in lines[start - 1:n - 1]):
                out.update(range(start, n))
            start = n + 1
    return out


def claim_paths() -> tuple[list[tuple[str, int, str]], int, set[str]]:
    """(document, line, path) for unresolved paths; record count; NOT_OURS seen."""
    bad: list[tuple[str, int, str]] = []
    records = 0
    seen: set[str] = set()
    for doc in CLAIM_DOCS:
        p = ROOT / doc
        if not p.is_file():
            bad.append((doc, 0, "<the document itself is missing>"))
            continue
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        skip = record_lines(lines)
        for n, line in enumerate(lines, 1):
            for tok in CLAIM_RE.findall(line):
                tok = tok.strip()
                if "/" not in tok or not PATH_RE.match(tok):
                    continue
                if tok in NOT_OURS:
                    seen.add(tok)
                    continue
                if (ROOT / tok.rstrip("/")).exists():
                    continue
                if n in skip:
                    records += 1
                    continue
                bad.append((doc, n, tok))
    return bad, records, seen


def marker_paths() -> list[tuple[str, int, str]]:
    """(document, line number, path) for every `<!-- unwired: -->` in the tree."""
    out = []
    for md in sorted(ROOT.glob(MARKER_GLOB)):
        rel = md.relative_to(ROOT).as_posix()
        if any(rel.startswith(s) or f"/{s}" in f"/{rel}" for s in MARKER_SKIP):
            continue
        for n, line in enumerate(
            md.read_text(encoding="utf-8", errors="replace").splitlines(), 1
        ):
            for m in MARKER_RE.finditer(line):
                out.append((rel, n, m.group(1)))
    return out


def find_block(lines: list[str]) -> tuple[int, int] | None:
    """(first, last) line indices of the fenced tree body, exclusive of fences."""
    for i, line in enumerate(lines):
        if line.strip() == TREE_HEADER:
            top = i
            while top > 0 and not lines[top].strip().startswith("```"):
                top -= 1
            bot = i
            while bot < len(lines) - 1 and not lines[bot].strip().startswith("```"):
                bot += 1
            if lines[top].strip().startswith("```") and lines[bot].strip().startswith("```"):
                return top + 1, bot
    return None


def parse(body: list[str]) -> list[tuple[int, str, str]]:
    """(line number within the block, depth, entry) for each path the tree draws.

    Depth is the count of tree columns to the left of the entry, which is what
    the drawing already encodes; comment-only continuation lines carry no entry
    and are skipped.
    """
    out = []
    for n, line in enumerate(body):
        head = line.split("#", 1)[0]
        stripped = head.lstrip(GLYPHS)
        if not stripped.strip():
            continue
        m = ENTRY_RE.match(stripped.strip())
        if not m:
            continue
        prefix = head[: len(head) - len(stripped)]
        depth = len(re.findall(r"[│|├└`]", prefix))
        out.append((n, depth, m.group(1)))
    return out


def resolve(entries: list[tuple[int, int, str]]) -> list[tuple[int, str]]:
    """Attach each entry to its parent by depth, returning repo-relative paths."""
    parent: dict[int, str] = {}
    out = []
    for n, depth, name in entries:
        # The header is the repository root, not a directory inside it: it
        # anchors depth 0 to the empty prefix. Without this every path below
        # resolves to `Oh-Ben-Claw/...` and the check reports the entire tree
        # missing — which is how the first run of it failed.
        if name.rstrip("/") == TREE_HEADER.rstrip("/"):
            parent[depth] = ""
            continue
        base = parent.get(depth - 1, "")
        rel = f"{base}{name}"
        if name.endswith("/"):
            parent[depth] = rel
            for deeper in [d for d in parent if d > depth]:
                del parent[deeper]
        out.append((n, rel))
    return out


def main() -> int:
    if not README.is_file():
        print("!! no README.md", file=sys.stderr)
        return 2

    lines = README.read_text(encoding="utf-8", errors="replace").splitlines()
    block = find_block(lines)
    if block is None:
        print(f"!! no fenced block starting `{TREE_HEADER}` in README.md — the "
              f"tree moved,\n   was renamed, or was deleted. This check cannot "
              f"pass by not finding it.", file=sys.stderr)
        return 2

    top, bot = block
    body = lines[top:bot]
    paths = resolve(parse(body))
    if not paths:
        print("!! parsed 0 entries from the tree block — the parser is wrong, "
              "not the tree", file=sys.stderr)
        return 2

    # The header line itself resolves to the repo root; drop it.
    paths = [(n, p) for n, p in paths if p.rstrip("/") != TREE_HEADER.rstrip("/")]

    missing = [(n, p) for n, p in paths if not (ROOT / p.rstrip("/")).exists()]

    listed_children: dict[str, set[str]] = {}
    for _, p in paths:
        parts = p.rstrip("/").split("/")
        if len(parts) == 2 and parts[0] in COMPLETE:
            listed_children.setdefault(parts[0], set()).add(parts[1])

    incomplete: list[tuple[str, list[str]]] = []
    for parent, listed in listed_children.items():
        actual = {
            d.name for d in (ROOT / parent).iterdir()
            if d.is_dir() and not d.name.startswith(".")
        } - COMPLETE[parent]
        gap = sorted(actual - listed)
        if gap:
            incomplete.append((parent, gap))

    markers = marker_paths()
    stale_markers = [
        (doc, n, p) for doc, n, p in markers if not (ROOT / p).exists()
    ]

    stale_claims, records, seen_not_ours = claim_paths()
    uncited, cited_count = cited_tests()

    # A suppression that no longer suppresses anything. Left in place it grows
    # into the reason a real stale path is invisible, so it fails here rather
    # than being quietly tidied.
    unused = sorted(NOT_OURS - seen_not_ours)
    for tok in unused:
        stale_claims.append(
            ("scripts/check_tree.py", 0,
             f"<NOT_OURS entry `{tok}` is claimed by no document — remove it>")
        )

    for led in LEDGERS:
        if led in CLAIM_DOCS:
            stale_claims.append(
                ("scripts/check_tree.py", 0,
                 f"<`{led}` is a ledger and cannot be a claim document>")
            )

    print(f"{len(paths)} path(s) drawn in README.md's tree (block at line "
          f"{top + 1}), {len(markers)} unwired marker(s)")
    print(f"{len(CLAIM_DOCS)} present-tense document(s) checked for backticked "
          f"repository paths\n    ({records} skipped as records: blockquotes "
          f"and dated paragraphs)")
    print(f"{cited_count} test(s) cited by {SAFETY_CASE}'s safety controls")

    if (not missing and not incomplete and not stale_markers
            and not stale_claims and not uncited):
        print("ok: every path the tree draws exists, `crates/` is listed in "
              "full, every\n    unwired marker names a file that is here, every "
              "path a present-tense\n    document offers as evidence can be "
              "opened, and every test the safety\n    case cites is one you can "
              "run")
        return 0

    if missing:
        print("\n── Drawn on the front page, absent from the repository ──")
        for n, p in missing:
            print(f"  README.md:{top + n + 1}  {p}")

    for parent, gap in incomplete:
        print(f"\n── `{parent}/` is listed partially, which reads as completely ──")
        print(f"  {len(gap)} not drawn: {', '.join(gap)}")

    if stale_markers:
        print("\n── An `unwired:` marker naming a file that is not here ──")
        for doc, n, p in stale_markers:
            print(f"  {doc}:{n}  {p}")
        print(f"{'':<2}file_reachability.py matches disclosures by this path. A "
              f"marker that\n{'':<2}resolves to nothing discloses nothing, and "
              f"the survey will report the\n{'':<2}component as undisclosed "
              f"while the document beside it explains itself.")

    if stale_claims:
        print("\n── Offered as evidence by a present-tense document, not here ──")
        for doc, n, p in stale_claims:
            where = f"{doc}:{n}" if n else doc
            print(f"  {where:<44}{p}")
        print(f"{'':<2}These are not broken links. A path in one of these "
              f"documents is the\n{'':<2}evidence for the sentence around it, "
              f"and a reader who cannot open it is\n{'':<2}left with the "
              f"sentence alone — which is the state the document exists to\n"
              f"{'':<2}improve on. If the claim is about another repository, "
              f"add the path to\n{'':<2}NOT_OURS with a reason; if the file "
              f"moved, follow it.")

    if uncited:
        print(f"\n── Cited as evidence by a safety control, and not a test ──")
        for n, name in uncited:
            print(f"  {SAFETY_CASE}:{n}  {name}")
        print(f"{'':<2}A safety case exists to be checkable by someone who does "
              f"not trust it.\n{'':<2}A control that cites a test nobody can run "
              f"is back to being an assertion,\n{'':<2}and it fails quietly: a "
              f"renamed test still passes, so nothing goes red.")

    print(f"\n{len(missing) + len(incomplete) + len(stale_markers) + len(stale_claims) + len(uncited)} "
          f"problem(s). A tree that names a directory the repository\ndoes not "
          f"have is not out of date in a way a reader can see — every line of "
          f"it\nlooks exactly as correct as the lines that are.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
