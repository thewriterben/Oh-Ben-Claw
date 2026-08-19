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

# ── Rule 4: backticked repo paths in the present-tense documents ────────────
# The documents that describe the system *now*. Not a glob, and the omissions
# are the argument: ROADMAP.md and docs/ACCELERAPP-CROSS-POLLINATION.md are
# append-only ledgers, and an entry dated 2026-07-30 describing work done when
# `src/spine/mod.rs` existed is not lying. Holding a record to today's tree
# would demand falsifying it, and a gate that demands that gets turned off.
#
# On 2026-08-19, 215 of 375 backticked repo paths across the eight claim
# documents did not resolve — ROADMAP.md alone accounted for 138 of them, which
# is why the split matters more than the total.
PRESENT_TENSE_DOCS = (
    "README.md",
    "docs/architecture/ARCHITECTURE.md",
    "docs/EMBODIED-ARCHITECTURE.md",
    "docs/SAFETY-CASE.md",
    "docs/SUBSYSTEM-SUITES-STATUS.md",
)

# Even inside those, two shapes are records and are skipped:
#   * a blockquote — this repository's convention for a correction, and the
#     paragraph that says "this used to claim X" must be free to say `X`
#   * a paragraph carrying a date — the same escape hatch check_counts.py
#     already offers with "add a date to the line if it is a historical record"
DATE_RE = re.compile(r"\b20\d\d-\d\d-\d\d\b")
BACKTICK_PATH_RE = re.compile(r"`([A-Za-z0-9_][A-Za-z0-9_./-]*)`")


def _record_lines(lines: list[str]) -> list[bool]:
    """Which lines belong to a paragraph that describes the past."""
    flags = [False] * len(lines)
    start = 0
    for i in range(len(lines) + 1):
        if i == len(lines) or not lines[i].strip():
            para = lines[start:i]
            if para and (
                all(l.lstrip().startswith(">") or not l.strip() for l in para)
                or DATE_RE.search("\n".join(para))
            ):
                for j in range(start, i):
                    flags[j] = True
            start = i + 1
    return flags


def stale_doc_paths() -> list[tuple[str, int, str]]:
    """(document, line, path) for each present-tense path that does not resolve."""
    tops = tuple(
        sorted(p.name for p in ROOT.iterdir() if p.is_dir() and not p.name.startswith("."))
    )
    out = []
    for rel in PRESENT_TENSE_DOCS:
        p = ROOT / rel
        if not p.is_file():
            continue
        lines = p.read_text(encoding="utf-8", errors="replace").splitlines()
        record = _record_lines(lines)
        for i, line in enumerate(lines):
            if record[i]:
                continue
            for m in BACKTICK_PATH_RE.finditer(line):
                c = m.group(1)
                if not c.startswith(tops) or "/" not in c:
                    continue
                if not (ROOT / c.rstrip("/")).exists():
                    out.append((rel, i + 1, c))
    return out



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
    stale_docs = stale_doc_paths()

    print(f"{len(paths)} path(s) drawn in README.md's tree (block at line "
          f"{top + 1}), {len(markers)} unwired marker(s)")

    if not missing and not incomplete and not stale_markers and not stale_docs:
        print("ok: every path the tree draws exists, `crates/` is listed in "
              "full, every\n    unwired marker names a file that is here, and "
              "the present-tense documents\n    point at directories that exist")
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

    if stale_docs:
        print("\n── A present-tense document pointing at a path that is not here ──")
        for doc, n, p in stale_docs:
            print(f"  {doc}:{n}  {p}")
        print(f"{'':<2}These documents describe the system now. A record may name a "
              f"path that has\n{'':<2}gone — put it in a blockquote, or date the "
              f"paragraph, which is what the\n{'':<2}rest of this repository "
              f"already does.")

    print(f"\n{len(missing) + len(incomplete) + len(stale_markers) + len(stale_docs)} "
          f"problem(s). A tree that names a directory the repository\ndoes not "
          f"have is not out of date in a way a reader can see — every line of it"
          f"\nlooks exactly as correct as the lines that are.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
