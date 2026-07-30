"""Constructed, wired, and never interrogated — the third shape.

Why this exists
---------------
Two surveys already ask whether code is *reachable*:

    scripts/curation_survey.py     is this module referenced?
    scripts/file_reachability.py   is this file referenced?

Both cleared `src/security/pairing.rs`, correctly. `NodePairingManager` is
referenced: it is a field on `SecurityManager` and it is constructed at startup.
What never happens is anyone asking it anything — `pair_node` has no callers,
`is_trusted` has none, the `pairing` field is never read, and
`security.require_pairing` is consulted only by config validation. Reachable,
instantiated, unit-tested, and inert.

That was the third control found in this shape in one week, after the tool
sandbox and ClawHub's install policy. All three had tests. All three had config
keys. A control with tests and a config key reads as present to every check
except running it, and neither reachability survey can see the difference,
because the type genuinely is reachable.

    python scripts/inert_components.py            # suspects only
    python scripts/inert_components.py --all      # every field examined

What it looks for
-----------------
A struct field whose type is defined in this crate, which is written by a
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
  * It cannot see a field that is read but only to pass along and drop, which is
    inertness one level up.
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
    for f in SRC.rglob("*.rs"):
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
files = sorted(SRC.rglob("*.rs"))
corpus = [(f, read(f)) for f in files]

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
        # have no leading dot, so `.field` matches reads and nothing else.
        pat = re.compile(rf"\.{re.escape(field)}\b")
        reads = sum(len(pat.findall(t)) for _, t in corpus)
        methods = inherent_methods("".join(t for _, t in corpus), bare)
        usable = {m for m in methods if m not in TOO_GENERIC and len(m) > 3}
        called = 0
        for m in usable:
            mp = re.compile(rf"\.{re.escape(m)}\s*\(")
            called += sum(len(mp.findall(t)) for _, t in corpus)

        rows.append({
            "file": f.relative_to(ROOT).as_posix(), "owner": owner,
            "field": field, "ty": bare, "reads": reads,
            "methods": len(usable), "calls": called,
        })

suspects = [r for r in rows if r["reads"] == 0]

print(f"{len(rows)} field(s) of crate-local type examined "
      f"(generic and short names skipped).\n")
print("── Written by a constructor, never read elsewhere ──")
if suspects:
    print(f"{'field':<26}{'type':<26}{'owner':<24}{'methods called':>15}")
    for r in sorted(suspects, key=lambda r: r["calls"]):
        loc = f"{r['owner']} ({r['file'].removeprefix('src/')})"
        flag = "  <-- INERT" if r["calls"] == 0 else ""
        print(f"{'.' + r['field']:<26}{r['ty']:<26}{loc:<24}"
              f"{r['calls']:>7} of {r['methods']:<3}{flag}")
    inert = [r for r in suspects if r["calls"] == 0]
    print(f"\n{len(suspects)} unread field(s); {len(inert)} whose type also has no "
          f"method called anywhere.")
    if inert:
        print("Those are the ones to read: built, stored, and never asked anything.")
else:
    print("none")

if show_all:
    print("\n── Every field examined ──")
    for r in sorted(rows, key=lambda r: (r["reads"], r["calls"])):
        print(f"  .{r['field']:<22}{r['ty']:<26}reads={r['reads']:<5}"
              f"method calls={r['calls']}")

print("\nName-based, not a compiler. A field this clears is not proven live, and a")
print("field it flags is worth reading rather than deleting. See the module docs.")
