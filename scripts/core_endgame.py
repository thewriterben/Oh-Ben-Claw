"""What would it take to extract the core, and is there an order at all?

`scripts/extractability.py` ranks modules by blocking-edge *count*. That is the
right question for the cheap end of the queue, where an edge is usually one type
in the wrong place. It is the wrong question for the core, for two reasons:

  1. A count cannot tell a 39-line turn from a structural dependency. movement's
     one edge was a struct field; config's six are not.
  2. A count cannot see a **cycle**. If agent needs tools and tools needs agent,
     no ordering of extractions exists, and every "extract A first" plan is
     wrong before it is written.

So this reports, for the modules that are still in the tree: the edges as a
directed graph, the cycles in it, and for each edge the distinct symbols crossed
with their occurrence counts. The symbol list is what turns "6 blocking edges"
into an estimate: one symbol crossed twice is a NodeState; forty symbols crossed
three hundred times is a rewrite.

Grep-derived, and therefore a ranking rather than a verdict -- same caveat the
sibling scripts carry. The compiler decides; this decides what to ask it.
"""
import re
from pathlib import Path

SRC = Path("src")
lib = (SRC / "lib.rs").read_text(encoding="utf-8")

# Names that have already left for a crate, and are therefore free to depend on.
#
# Every pattern here must anchor on `obc_`. The first one did not until
# 2026-08-13 — it read `^pub use (?:obc_\w+ as )?(\w+)`, where the group is
# optional, so `pub use config::Config;` on the last line of lib.rs matched and
# put `config` in this set. `config` is not a crate. It is 3371 lines sitting in
# the middle of the tree, and for as long as that regex stood, **every edge
# pointing into it was invisible to this script**: `spine -> config`,
# `agent -> config`, `gateway -> config`, `tools -> config`, all dropped, while
# `config -> agent` and the rest were counted normally.
#
# That asymmetry is the worst possible shape for a cycle detector. A mutual pair
# through `config` could not be seen from one side, so it could not be seen at
# all. Every cycle count and crossing total this script printed before this date
# was low by an unknown amount, and the "no cycles remain" it printed on
# 2026-08-13 was wrong.
#
# Found by two instruments disagreeing: `extractability.py` said `spine` had one
# blocking edge and this said zero. Neither was checked against the other until
# the disagreement made it necessary. The lesson is not "regexes are hard" — it
# is that a measurement nothing cross-checks is a claim, and this document's
# whole argument rests on the difference.
gone = set(re.findall(r"^pub use (obc_\w+);", lib, re.M))
gone |= set(re.findall(r"^pub use obc_\w+ as (\w+);", lib, re.M))
gone |= {m for m in re.findall(r"^pub use obc_\w+::\{([^}]*)\}", lib, re.M)
         for m in (x.strip() for x in m.split(","))}
gone |= set(re.findall(r"^pub use obc_\w+::(\w+);", lib, re.M))
here = sorted(set(re.findall(r"^pub mod (\w+);$", lib, re.M)))

# The cheapest guard that would have caught the bug above, and did not exist for
# the five weeks it was live: a name cannot be both extracted and still here.
# `config` was in both from the first run of this script.
both = sorted(set(gone) & set(here))
assert not both, (
    f"{both} are listed as both extracted and still in the tree. "
    "One of the two patterns above is over-matching; every edge into these "
    "modules is being silently dropped."
)


def files_of(m: str) -> list[Path]:
    d = SRC / m
    return sorted(d.rglob("*.rs")) if d.is_dir() else [p for p in [SRC / f"{m}.rs"] if p.exists()]


# edges[a][b] = {symbol: count} for a -> b, both still in the tree
edges: dict[str, dict[str, dict[str, int]]] = {}
loc: dict[str, int] = {}
for m in here:
    fs = files_of(m)
    if not fs:
        continue
    loc[m] = sum(f.read_text(encoding="utf-8", errors="replace").count("\n") for f in fs)
    out: dict[str, dict[str, int]] = {}
    for f in fs:
        text = f.read_text(encoding="utf-8", errors="replace")
        for target, sym in re.findall(r"\bcrate::(\w+)::(\w+)", text):
            if target == m or target in gone or target not in here:
                continue
            out.setdefault(target, {})
            out[target][sym] = out[target].get(sym, 0) + 1
    edges[m] = out

# Cycles, by DFS over the edge graph.
cycles: list[list[str]] = []
seen_pairs: set[tuple[str, ...]] = set()


def walk(node: str, path: list[str]) -> None:
    for nxt in edges.get(node, {}):
        if nxt in path:
            cyc = path[path.index(nxt):] + [nxt]
            key = tuple(sorted(set(cyc)))
            if key not in seen_pairs:
                seen_pairs.add(key)
                cycles.append(cyc)
        elif len(path) < 4:
            walk(nxt, path + [nxt])


for m in edges:
    walk(m, [m])

print(f"{len(here)} modules still in the tree; {len(gone)} names already extracted\n")

print("=== CYCLES (no extraction order exists through these) ===")
if not cycles:
    print("  none\n")
for c in sorted(cycles, key=len):
    print("  " + " -> ".join(c))
print()

CORE = ["agent", "tools", "config", "gateway", "spine"]
print("=== THE CORE, EDGE BY EDGE ===")
for m in CORE:
    if m not in edges:
        continue
    tot = sum(sum(v.values()) for v in edges[m].values())
    print(f"\n{m}  ({loc[m]} loc, {len(edges[m])} blocking edges, {tot} crossings)")
    for target, syms in sorted(edges[m].items(), key=lambda kv: -sum(kv[1].values())):
        n = sum(syms.values())
        top = " ".join(f"{s}x{c}" if c > 1 else s
                       for s, c in sorted(syms.items(), key=lambda kv: -kv[1])[:8])
        back = "  <-- MUTUAL" if m in edges.get(target, {}) else ""
        print(f"    -> {target:<12} {len(syms):>3} symbols, {n:>4} crossings{back}")
        print(f"       {top}")
