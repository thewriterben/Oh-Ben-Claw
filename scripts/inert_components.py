"""Constructed, wired, and never interrogated — the third shape.

Why this exists
---------------
Two surveys already ask whether code is *reachable*:

    scripts/curation_survey.py     is this module referenced?
    scripts/file_reachability.py   is this file referenced?

Both cleared `src/security/pairing.rs`, correctly. `NodePairingManager` is
referenced: it is a field on `SecurityContext` and it is constructed at startup.
What never happens is anyone asking it anything — `pair_node` has no callers,
`is_trusted` has none, the `pairing` field is never read, and
`security.require_pairing` is consulted only by config validation. Reachable,
instantiated, unit-tested, and inert.

(That file is now `crates/obc-safety/src/lib.rs`; the finding did not move with
it, and neither did the survey until 2026-08-14. See `workspace_files` below.)

That was the third control found in this shape in one week, after the tool
sandbox and ClawHub's install policy. All three had tests. All three had config
keys. A control with tests and a config key reads as present to every check
except running it, and neither reachability survey can see the difference,
because the type genuinely is reachable.

    python scripts/inert_components.py            # suspects, tier 3 truncated
    python scripts/inert_components.py --all      # every field, nothing withheld

What it looks for
-----------------
A struct field whose type is defined in the workspace, which is written by a
constructor and **never read anywhere else**. That is the shape of an object
built and parked. It is a narrower question than "is this type used", and a much
more precise one: field names are distinctive where method names are not.
Matching `.revoke(` would have told us `NodePairingManager` was in use, because
approval grants have a `revoke` too — the field `.pairing` had no such twin.

Secondarily, for each flagged field it reports whether the type's own inherent
methods are called anywhere, as corroboration rather than as the test.

Limits, stated plainly
----------------------
  * Name-based, like its siblings. Not a compiler, no type resolution. A field
    called `.state` or `.config` will collide with unrelated code and read as
    live — false *negatives* are the failure mode.
  * Fields consumed only through a trait object, a macro, or serde round-tripping
    will look unread. Check before believing it.
  * A field whose type contains a top-level comma is not examined at all, and
    is not counted as an owner of its name either. `EdgeAgentBuilder` declares
    `pairing: (NodePairingManager, bool)` and this survey does not know it
    exists. Found by checking why `.pairing` landed in tier 2 rather than
    tier 3; left as a limit rather than fixed, because matching a balanced type
    expression with a regex is how the next wrong answer gets written.
  * It cannot see a field that is read but only to pass along and drop, which is
    inertness one level up.
  * There are three verdicts, not one, and only the first is a finding. Across
    202 files 133 of the 218 fields examined have a name some other type also
    declares, so a bare `.field` count usually cannot say *whose* field was
    read — nor reliably whether it was read or written. Tiers 2 and 3 are that
    admission, printed rather than swallowed — see the comment above the tier
    split for what each one means and which way it fails.
  * A clean run is not a clean bill. It is one more question answered, and the
    reason this file exists is that the previous two clean bills were also true
    and also insufficient.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
CRATES = ROOT / "crates"

# Same fix as file_reachability.py, for the same reason and found the same way:
# the section rules below are box characters, a Windows console defaults to
# cp1252, and printing one raises UnicodeEncodeError *after* the survey has
# already done all its work. It reads as a broken tool and is a broken
# terminal.
#
# This comment used to say the guard "should be copied into the next one before
# it crashes rather than after". `curation_survey.py` crashed on it the next
# day. Advice is not a mechanism, so it is scripts/console.py now.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402

use_utf8_stdout()


def workspace_files() -> list[Path]:
    """Every Rust source in the workspace, binary and crates alike.

    This scanned `src/` alone until 2026-08-14, which was right when `src/` was
    the codebase. It is now ten files against a hundred and ninety-two: the
    extraction moved the subject out from under the instrument, and a survey
    that cannot see its subject reports clean, which reads as health.

    `NodePairingManager` — the example in this module's own docstring, the
    control that was reachable, instantiated, unit-tested and inert — has been
    in obc-safety since July. This survey has not been able to see it for
    weeks, while still printing its name in the header as the reason it exists.

    Widening the corpus widens both halves at once, which is why it is one
    change and not two: the subjects to examine, and the call sites that count
    as evidence. Narrowing either alone would invent inertness that is not
    there — a method called from another crate is called.
    """
    out = sorted(SRC.rglob("*.rs"))
    if CRATES.is_dir():
        out += sorted(CRATES.glob("*/src/**/*.rs"))
    return out

# Field names too common for a name-based reader to say anything about.
TOO_GENERIC = {
    "config", "state", "inner", "data", "value", "name", "id", "kind", "path",
    "conn", "db", "store", "client", "handle", "tx", "rx", "sender", "receiver",
    "tools", "agent", "provider", "memory", "world", "log", "logger", "metrics",
    "options", "opts", "params", "args", "buf", "buffer", "cache", "index",
}

STRUCT = re.compile(r"^\s*pub struct\s+([A-Z][A-Za-z0-9_]*)\s*\{", re.M)
FIELD = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?([a-z_][a-z0-9_]*)\s*:\s*([^,]+),\s*$")
TYPE_DECL = re.compile(r"^\s*pub (?:struct|enum)\s+([A-Z][A-Za-z0-9_]*)", re.M)
IMPL_FN = re.compile(r"^\s*pub (?:async )?fn\s+([a-z_][a-z0-9_]*)", re.M)


def read(p: Path) -> str:
    try:
        return p.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def local_types() -> set[str]:
    out: set[str] = set()
    for f in workspace_files():
        out.update(TYPE_DECL.findall(read(f)))
    return out


def struct_fields(text: str) -> list[tuple[str, str, str]]:
    """(struct_name, field_name, field_type) for module-level pub structs."""
    out = []
    for m in STRUCT.finditer(text):
        name = m.group(1)
        depth, i = 0, m.end() - 1
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        for line in text[m.end():i].splitlines():
            fm = FIELD.match(line)
            if fm:
                out.append((name, fm.group(1), fm.group(2).strip()))
    return out


def inherent_methods(text: str, ty: str) -> set[str]:
    """`pub fn` names inside `impl Ty {` blocks, minus constructor shapes."""
    out: set[str] = set()
    for m in re.finditer(rf"^impl(?:<[^>]*>)?\s+{re.escape(ty)}\b[^{{]*\{{", text, re.M):
        depth, i = 0, m.end() - 1
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        for fn in IMPL_FN.findall(text[m.end():i]):
            if fn in ("new", "default") or fn.startswith(("with_", "from_", "build")):
                continue
            out.add(fn)
    return out


types = local_types()
files = workspace_files()
corpus = [(f, read(f)) for f in files]

# Which types declare a field of each name, across the whole workspace.
#
# This is the cost of widening, and it has to be paid explicitly. `.field` was
# a precise signal inside one crate: a dot prefix matches reads and not
# declarations, and within ten files a name collision was unlikely. Across two
# hundred it is normal, and a collision does not merely add noise -- it points
# the wrong way. Every stray namesake read makes a genuinely inert field look
# live, and the survey prints "none" with more authority than before.
#
# The first run after widening did exactly that. It examined 218 fields, found
# nothing, and the nothing included `SecurityContext.pairing` -- the control in
# this module's own docstring, still inert, masked by `self.pairing` on an
# unrelated builder in obc-agent. A clean bill that no longer contains the case
# the tool was written for is not a clean bill.
#
# So a name owned by more than one type is not evidence either way. It is
# reported as unattributable rather than counted as read, because the honest
# answer here is "this method cannot tell" and the dangerous one is silence.
owners_of_field: dict[str, set[str]] = {}
for _f, text in corpus:
    for owner, field, _fty in struct_fields(text):
        owners_of_field.setdefault(field, set()).add(owner)

show_all = "--all" in sys.argv
rows = []

for f, text in corpus:
    for owner, field, fty in struct_fields(text):
        bare = re.sub(r"^(?:Option|Arc|Box|Rc|Mutex|RwLock)<", "", fty)
        bare = re.sub(r"[<>&\s].*$", "", bare).strip()
        if bare not in types or field in TOO_GENERIC or len(field) < 4:
            continue

        # Count `.field` across the WHOLE crate, its own file included.
        #
        # A first version excluded the defining file, on the theory that external
        # use is what matters. That is exactly backwards: a field is almost always
        # read as `self.field` inside its own impl block, so excluding that file
        # made nearly every field look unread and buried the real signal in
        # twenty-six false positives. The dot prefix is what makes this precise —
        # a struct-literal initialiser (`field: value`) and the declaration itself
        # have no leading dot, so most of what `.field` matches is a read.
        #
        # Not all of it. `self.field = x` in a builder is a dotted *write*, and
        # `.pairing` is exactly that case: `EdgeAgentBuilder::with_pairing` in
        # obc-agent assigns `self.pairing`, and before the corpus widened that
        # line was in a different crate and invisible. Distinguishing the two
        # would mean parsing the right-hand side, so the name of the count stays
        # honest instead: it is `reads` in the sense of "mentions with a dot",
        # and the tier a field lands in is what carries the actual claim.
        pat = re.compile(rf"\.{re.escape(field)}\b")
        reads = sum(len(pat.findall(t)) for _, t in corpus)

        # Reads in files that also name the owning type.
        #
        # Plain `.field` counts every namesake in the workspace, and once the
        # corpus went from ten files to two hundred that stopped being a
        # detail: 49 of 134 distinct field names are declared by more than one
        # type, which is 133 of the 218 fields examined.
        # Reporting all of those as "cannot attribute" is technically true and
        # useless -- a list of 133 rows is a list nobody reads, which is the
        # same outcome as printing nothing, reached more expensively.
        #
        # A read in a file that never mentions `SecurityContext` is almost
        # certainly not a read of `SecurityContext.pairing`. That is a weaker
        # claim than a compiler's and a much stronger one than a bare name
        # match, and it is the difference between a survey with 133 maybes and
        # one with a handful.
        near = sum(len(pat.findall(t)) for _f, t in corpus if owner in t)
        methods = inherent_methods("".join(t for _, t in corpus), bare)
        usable = {m for m in methods if m not in TOO_GENERIC and len(m) > 3}
        called = 0
        for m in usable:
            mp = re.compile(rf"\.{re.escape(m)}\s*\(")
            called += sum(len(mp.findall(t)) for _, t in corpus)

        rows.append({
            "file": f.relative_to(ROOT).as_posix(), "owner": owner,
            "field": field, "ty": bare, "reads": reads, "near": near,
            "methods": len(usable), "calls": called,
            "shared": sorted(owners_of_field.get(field, {owner}) - {owner}),
        })

# Three tiers, because two different things can make a read invisible and they
# fail in opposite directions.
#
#   reads == 0              Nothing anywhere writes `.field`. Strong, and the
#                           same claim this survey has always made.
#
#   reads > 0, near == 0    Read somewhere, but nowhere that names the owning
#                           type. Two very different causes and no textual way
#                           to tell them apart:
#                             - a namesake on another type (the read is not
#                               this field's), or
#                             - a path expression -- `config.channels.imessage`
#                               in main.rs, which never says `ChannelsConfig`.
#                           The first means inert; the second means live.
#
#   shared name, near > 0   Read in files that do name the type, but the name
#                           is not unique. Least suspicious; listed last.
#
# An earlier version of this widening called tier 2 INERT and produced three
# false positives on the first run -- `.imessage`, `.matrix`, `.lora_serial`,
# all read from main.rs through `config.` paths. A false INERT is worse than
# noise: noise gets ignored, and a confident wrong flag gets acted on.
inert = [r for r in rows if r["reads"] == 0]
type_blind = [r for r in rows if r["reads"] and not r["near"]]
unattributable = [r for r in rows if r["shared"] and r["near"] and r["reads"] != r["near"]]

print(f"{len(rows)} field(s) of crate-local type examined "
      f"(generic and short names skipped).\n")
print("── Written by a constructor, never read anywhere ──")
if inert:
    print(f"{'field':<20}{'type':<24}{'owner':<22}{'methods called':>15}")
    for r in sorted(inert, key=lambda r: r["calls"]):
        flag = "  <-- INERT" if r["calls"] == 0 else ""
        print(f"{'.' + r['field']:<20}{r['ty'][:23]:<24}{r['owner'][:21]:<22}"
              f"{r['calls']:>7} of {r['methods']:<3}{flag}")
        print(f"{'':<20}{r['file']}")
    dead = [r for r in inert if r["calls"] == 0]
    print(f"\n{len(inert)} unread field(s); {len(dead)} whose type also has no "
          f"method called anywhere.")
    if dead:
        print("Those are the ones to read: built, stored, and never asked anything.")
else:
    print("none")

print("\n── Read, but never in a file that names the owning type ──")
if type_blind:
    print(f"{'field':<20}{'type':<24}{'owner':<22}{'reads':>6}")
    for r in sorted(type_blind, key=lambda r: r["reads"]):
        print(f"{'.' + r['field']:<20}{r['ty'][:23]:<24}{r['owner'][:21]:<22}"
              f"{r['reads']:>6}")
        print(f"{'':<20}{r['file']}")
    print(f"\n{len(type_blind)} field(s). Each is either a namesake on another "
          "type — meaning\nthis one is inert — or a real read through a path "
          "expression like\n`config.channels.imessage`, where the type is never "
          "written down. This\nmethod cannot tell those apart; open the read "
          "sites and look.")
else:
    print("none")

# The tier-3 list is 113 rows long, and a 113-row list is not read.
#
# The rejected fix was to drop the tier entirely, on the grounds that it is the
# least suspicious one. The reason not to is in this tier's own name: these are
# the fields the method cannot speak to, so deleting them from the output turns
# "cannot tell" into silence, and silence here is indistinguishable from
# "checked, fine". That is the failure this whole module was written against.
#
# So: show the low-read end, where a genuinely inert field could plausibly be
# hiding, and say out loud how many were withheld and how to see them. A cap
# that announces itself is a cap; one that does not is a wrong answer.
TIER3_SHOWN = 15

print("\n── Field name owned by more than one type: cannot attribute reads ──")
if unattributable:
    ranked = sorted(unattributable, key=lambda r: (r["reads"], r["field"]))
    shown = ranked if show_all else ranked[:TIER3_SHOWN]
    print(f"{'field':<20}{'type':<24}{'owner':<22}{'also on':<30}{'reads':>6}")
    for r in shown:
        also = ", ".join(r["shared"])
        if len(also) > 29:
            also = also[:26] + "..."
        print(f"{'.' + r['field']:<20}{r['ty'][:23]:<24}{r['owner'][:21]:<22}"
              f"{also:<30}{r['reads']:>6}")
        print(f"{'':<20}{r['file']}")
    hidden = len(ranked) - len(shown)
    if hidden:
        print(f"\n... and {hidden} more with {ranked[len(shown)]['reads']} or more "
              f"reads, not shown. Pass --all for the full list.")
    print(f"\n{len(unattributable)} field(s) share a name with another type's field.")
    print("A `.field` count cannot say which type was read, so these are neither")
    print("cleared nor flagged. Read the ones whose reads are low; a genuinely")
    print("inert field hides here exactly as well as it would in silence.")
else:
    print("none")

if show_all:
    print("\n── Every field examined ──")
    for r in sorted(rows, key=lambda r: (r["reads"], r["calls"])):
        print(f"  .{r['field']:<22}{r['ty']:<26}reads={r['reads']:<5}"
              f"method calls={r['calls']}")

print("\nName-based, not a compiler. A field this clears is not proven live, and a")
print("field it flags is worth reading rather than deleting. See the module docs.")
