"""Subsystem ledger: what each module claims, and what evidence backs it.

Why this exists
---------------
The migration policy is "each piece moves to Open Body Control when it is
defensible". That needs a list of pieces and, per piece, what would make it
defensible. Neither existed. `ROADMAP.md` had no row at all for nine modules —
including `navigation` (3,714 LOC), which `docs/SOTA-COMPARISON.md` benchmarks
against ROS 2 Nav2, slam_toolbox and AMCL. Work with a strong public claim and
no phase to carry its status is invisible to the process meant to gate it.

So this reports, per module, two things it can measure and then compares them:

  claim strength    none < documented < benchmarked-against-SOTA
  evidence strength unwired < none < unit < integration

A module whose claim outruns its evidence is not necessarily broken. It is
un-*assessed*, which under a migrate-when-defensible policy is the thing that
blocks it. Those rows are the work list.

What this cannot see
--------------------
Bench and hardware validation. No static analysis can tell you that a BME280
actually fired a reflex on a real board. Those are declared in EVIDENCE below,
with a citation, and the script prints them as declared rather than measured —
the distinction is the point. An undeclared module is not accused of anything;
it simply has no bench evidence on record.

    python scripts/subsystem_ledger.py            # markdown table
    python scripts/subsystem_ledger.py --gaps      # only the mismatches
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

# Which subsystem each `docs/SOTA-COMPARISON.md` section is a claim about.
#
# Declared rather than inferred, because inferring it does not work. A first pass
# scored claim strength by counting the module name in that document, and reported
# `cost`, `runtime` and `learning` as carrying SOTA claims — the words "compute
# cost", "runtime shields" and "machine learning" all appear there as ordinary
# prose. Three of the four flagged gaps were artifacts of the measurement. Path
# citations do not work either: that document contains no `src/` paths at all.
#
# Every `### ` heading under "Component-by-component" must appear here or this
# script exits non-zero, so adding a comparison forces you to say what it is about.
SOTA_SECTIONS: dict[str, tuple[str, ...]] = {
    "Navigation & planning": ("navigation",),
    "SLAM": ("navigation",),
    "Localization (belief state)": ("navigation",),
    "Task-level control (missions vs behavior trees)": ("mission",),
    "Reactive safety (Track 0 / safing)": ("security", "agent"),
    "Foresight (anticipatory layer)": ("foresight",),
    "Self-authored rules": ("learning",),
    "Dual-system control (System 1 / System 2)": ("agent",),
    "Fleet coordination": ("fleet",),
    "Frontier exploration": ("navigation",),
}

# Declared, not measured. Cite where the evidence lives; an empty dict is not an
# accusation, it is an absence of record. Add a row when you validate something.
EVIDENCE: dict[str, str] = {
    "spine": "bench-validated 2026-07-17 (2x Heltec V3 LoRa link, host<->mesh both directions)",
    "peripherals": "bench-validated 2026-07-17 (XIAO ESP32-S3 over serial; gpio_write pin=99 refused by the node allow-list)",
    "vision": "bench-validated on 14 days of recorded ClawCam detections (bodies/trailwatch)",
    "memory": "bench-validated on the live world store 2026-07-28 (10 beliefs withdrawn at one boot, mesh-node-lost 2376 -> 0)",
}

CLAIM_RANK = {"none": 0, "documented": 1, "benchmarked": 2}
EVID_RANK = {"unwired": -1, "none": 0, "unit": 1, "integration": 2, "bench": 3}


def use_tree_heads(text: str) -> set[str]:
    """Top-level module names imported via `use crate::{...}` / `use oh_ben_claw::{...}`.

    Brace-matched. Same logic as scripts/curation_survey.py, which had to learn
    it the hard way — a regex that missed grouped imports reported the HTTP
    gateway as removable.
    """
    heads: set[str] = set()
    for m in re.finditer(r"\buse\s+(?:crate|oh_ben_claw)\s*::\s*", text):
        i = m.end()
        if i >= len(text):
            continue
        if text[i] != "{":
            ident = re.match(r"([a-z_][a-z0-9_]*)", text[i:])
            if ident:
                heads.add(ident.group(1))
            continue
        depth, j = 0, i
        while j < len(text):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        group, depth, start, parts = text[i + 1:j], 0, 0, []
        for k, ch in enumerate(group):
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            elif ch == "," and depth == 0:
                parts.append(group[start:k])
                start = k + 1
        parts.append(group[start:])
        for part in parts:
            ident = re.match(r"\s*([a-z_][a-z0-9_]*)", part)
            if ident:
                heads.add(ident.group(1))
    return heads


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
        out.append(line)
    return "\n".join(out)


def read(p: Path) -> str:
    return p.read_text(encoding="utf-8", errors="replace") if p.exists() else ""


lib = read(SRC / "lib.rs")
if not lib:
    sys.exit("no src/lib.rs — run from the repo root")
declared = re.findall(r"^\s*pub\s+mod\s+([a-z_][a-z0-9_]*)\s*;", lib, re.M)
mods = sorted(m for m in declared if (SRC / m).is_dir())

consumers: list[tuple[Path, set[str], str]] = []
for extra in [SRC] + [ROOT / d for d in ("tests", "examples", "benches", "gui", "planner-wasm")]:
    if extra.is_dir():
        for f in extra.rglob("*.rs"):
            t = read(f)
            consumers.append((f, use_tree_heads(t), t))

# Stripped, so the ledger does not count the table it generates. See strip_audit.
roadmap = strip_audit(read(ROOT / "ROADMAP.md"))
readme = strip_audit(read(ROOT / "README.md"))
sota = read(ROOT / "docs" / "SOTA-COMPARISON.md")

# Fail loudly on an unmapped comparison rather than silently scoring it as no
# claim — a new SOTA section that nothing points at is the exact shape of the gap
# this script exists to surface.
sota_headings = re.findall(r"^### (.+?)\s*$", sota, re.M)
unmapped = [h for h in sota_headings if h not in SOTA_SECTIONS]
if unmapped:
    sys.exit("SOTA-COMPARISON.md has unmapped section(s); add them to SOTA_SECTIONS:\n  "
             + "\n  ".join(unmapped))
stale = [h for h in SOTA_SECTIONS if h not in sota_headings]
if stale and sota_headings:
    print("note: SOTA_SECTIONS names section(s) no longer in the document: "
          + ", ".join(stale), file=sys.stderr)
sota_modules = {mod for h in sota_headings for mod in SOTA_SECTIONS.get(h, ())}

rows = []
for m in mods:
    own = SRC / m
    loc = sum(len(read(f).splitlines()) for f in own.rglob("*.rs"))
    body = "".join(read(f) for f in own.rglob("*.rs"))
    tests = len(re.findall(r"#\[(?:tokio::)?test\]", body))

    direct = re.compile(rf"\b(?:crate|oh_ben_claw)\s*::\s*{re.escape(m)}\b")
    ext, suites = 0, []
    for f, heads, text in consumers:
        try:
            f.relative_to(own)
            continue
        except ValueError:
            pass
        if m in heads or direct.search(text):
            ext += 1
            if f.parent.name == "tests" or (ROOT / "tests") in f.parents:
                suites.append(f.stem)

    in_roadmap = len(re.compile(rf"\b{re.escape(m)}\b", re.I).findall(roadmap))

    claim = "benchmarked" if m in sota_modules else "documented"
    if ext == 0:
        evidence = "unwired"
    elif m in EVIDENCE:
        evidence = "bench"
    elif suites:
        evidence = "integration"
    elif tests:
        evidence = "unit"
    else:
        evidence = "none"

    rows.append({
        "mod": m, "loc": loc, "tests": tests, "ext": ext,
        "suites": sorted(set(suites)), "roadmap": in_roadmap,
        "claim": claim, "evidence": evidence,
        "gap": CLAIM_RANK[claim] - max(EVID_RANK[evidence], 0),
        "no_phase": in_roadmap == 0,
    })

gaps_only = "--gaps" in sys.argv
rows.sort(key=lambda r: (-r["gap"], -r["loc"]))

print("| module | LOC | tests | wired | integration suites | roadmap | claim | evidence |")
print("|---|---:|---:|---:|---|---:|---|---|")
for r in rows:
    if gaps_only and r["gap"] <= 0 and not r["no_phase"]:
        continue
    suites = ", ".join(f"`{s}`" for s in r["suites"]) or "—"
    flag = " ⚠" if r["gap"] > 0 else ""
    phase = str(r["roadmap"]) if r["roadmap"] else "**0**"
    print(f"| `{r['mod']}` | {r['loc']:,} | {r['tests']} | {r['ext']} | {suites} | {phase} "
          f"| {r['claim']} | {r['evidence']}{flag} |")

gaps = [r for r in rows if r["gap"] > 0]
nophase = [r for r in rows if r["no_phase"]]
print()
print(f"{len(rows)} modules, {sum(r['loc'] for r in rows):,} LOC, "
      f"{sum(r['tests'] for r in rows)} test fns.")
print(f"claim outruns evidence: {len(gaps)} module(s), {sum(r['loc'] for r in gaps):,} LOC"
      + (" — " + ", ".join(r["mod"] for r in gaps) if gaps else ""))
print(f"no ROADMAP presence:    {len(nophase)} module(s), {sum(r['loc'] for r in nophase):,} LOC"
      + (" — " + ", ".join(r["mod"] for r in nophase) if nophase else ""))
if EVIDENCE:
    print("\nDeclared bench/hardware evidence (not measured by this script):")
    for k, v in sorted(EVIDENCE.items()):
        mark = "" if k in mods else "  <- NOT A MODULE, stale declaration"
        print(f"  {k}: {v}{mark}")
undeclared_gap = [r["mod"] for r in gaps if r["mod"] not in EVIDENCE]
if undeclared_gap:
    print("\nThese carry a public claim with no bench evidence on record. Either add a")
    print("citation to EVIDENCE, soften the claim, or accept that they cannot yet be")
    print("said to be defensible: " + ", ".join(undeclared_gap))
