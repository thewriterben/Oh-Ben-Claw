"""Curation input: which modules are load-bearing, which are islands.

Not a dead-code detector. It answers one question — is this module referenced
from anywhere outside its own directory? — which is the cheapest honest proxy
for "would removing it break the build". A module with zero external references
is a candidate for the first public cut; it is not proof of deadness, and
anything with a `main.rs` reference is load-bearing regardless.
"""

import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
SRC = ROOT / "src"

mods = sorted(p.name for p in SRC.iterdir() if p.is_dir())

# Scan tests/ and examples/ too. An earlier version looked only at src/ and reported
# `a2a` as unreferenced; tests/evals.rs pins its wire shape as a release gate. An
# integration test is a consumer, and the one most likely to be the ONLY consumer of a
# protocol surface that has not shipped yet — precisely the case this is meant to find.
files = list(SRC.rglob("*.rs"))
for extra in ("tests", "examples", "benches"):
    d = ROOT / extra
    if d.is_dir():
        files += list(d.rglob("*.rs"))

rows = []
for m in mods:
    own = SRC / m
    pat = re.compile(rf"\b(?:crate|oh_ben_claw)::{re.escape(m)}\b|^\s*use\s+{re.escape(m)}::", re.M)
    ext, in_main = 0, False
    loc = 0
    for f in own.rglob("*.rs"):
        loc += sum(1 for _ in f.open(encoding="utf-8", errors="replace"))
    for f in files:
        try:
            f.relative_to(own)
            continue
        except ValueError:
            pass
        text = f.read_text(encoding="utf-8", errors="replace")
        if pat.search(text):
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
print("  " + ", ".join(r[0] for r in islands) if islands else "  none")
print(f"\ntotal: {len(rows)} modules, {sum(r[1] for r in rows)} LOC")
