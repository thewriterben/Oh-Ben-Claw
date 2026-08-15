"""Curation input: which modules are load-bearing, which are islands.

Not a dead-code detector. It answers one question — is this module referenced
from anywhere outside its own directory? — which is the cheapest honest proxy
for "would removing it break the build". A module with zero external references
is a candidate for the first public cut; it is not proof of deadness, and
anything with a `main.rs` reference is load-bearing regardless.

Two corrections, 2026-07-29
---------------------------
The previous version reported four islands — `gateway`, `tunnel`, `runtime`,
`bin` — and three of those were wrong.

1. **Grouped imports were invisible.** The pattern was

       \\b(?:crate|oh_ben_claw)::{mod}\\b | ^\\s*use\\s+{mod}::

   which cannot match `use oh_ben_claw::{config, gateway, …}` — the module name
   is inside a brace group, not after a `::`. That is how most of this crate is
   consumed. `gateway` (2,313 LOC, binds the whole HTTP API) and `tunnel` were
   both reported dead on the strength of it. A survey that reports the API
   gateway as removable is worse than no survey, because the one time it is
   believed is the time it does damage. Use-trees are now brace-matched and
   expanded, including nested ones (`{a, b::{c, d}}`).

2. **`src/bin/` is not a module.** It is Cargo's auto-discovered binary
   directory and is not declared in `src/lib.rs`, so "no external references" is
   its normal state. The module list now comes from `pub mod` declarations in
   `src/lib.rs` rather than from directory listing, which excludes it by
   construction and also means this script can never disagree with the crate
   about what a module is.

After both fixes the only genuine island was `runtime` (417 LOC) — see the
ROADMAP note against "Sandboxed tool execution". `runtime` has since been cut,
and the survey now reports no islands at all.

The import parser moved to `scripts/rust_imports.py` on 2026-08-01, when
`extractability.py` needed the same brace matcher. Two copies of it would have
been this script's own headline finding turned on itself: the 2026-07-29
correction took two wrong answers with it, and a second copy would have kept one
of them alive. Output is byte-identical across that refactor, which is how the
move was checked.

Consumers scanned: src/, tests/, examples/, benches/, gui/, planner-wasm/.

The second question, 2026-08-14
-------------------------------
`src/lib.rs` used to declare thirty-odd `pub mod`s. It now declares three, and
the rest are `pub use obc_x as X;` re-exports of workspace crates. So this
survey's subject list quietly shrank from the codebase to 2045 lines of it, and
kept printing "zero external references: none" about the part it could still
see. Same failure as `inert_components.py` had until yesterday: the extraction
moved the subject out from under the instrument, and an instrument that cannot
see its subject reports clean.

The module question does not stretch to crates, so it is not stretched. "Is this
module referenced from outside its own directory" is a proxy for "would removing
it break the build", and at crate scope that proxy has an exact answer: cargo
knows. `cargo metadata` is asked instead of regexes, which is also why the
grouped-import bug that produced the 2026-07-29 corrections cannot recur here —
there is no import parsing on this path at all.

Two things that would otherwise read as islands, excluded by construction rather
than by name, in the same spirit as `src/bin/` above:

  * A package with a `bin` target is an entry point. Nothing is supposed to
    depend on `oh-ben-claw`; that is what makes it the binary.
  * A package with a `cdylib` or `staticlib` target is a build artifact for
    something outside this workspace. `obc-planner-wasm` is compiled by
    wasm-pack and consumed by a browser, so "no in-workspace dependents" is its
    normal state and not a finding.

Both are read off `targets[].kind`, so a package that becomes an entry point
later is excluded the day it does, without anyone remembering to edit a list.
"""

import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402
from rust_imports import declared_modules, direct_ref, use_tree_heads  # noqa: E402

use_utf8_stdout()

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
SRC = ROOT / "src"
LIB = SRC / "lib.rs"

# The crate's own view of what a module is. Directory listing would also pick up
# src/bin/ (a Cargo convention, not a module) and any scratch directory.
if not LIB.exists():
    sys.exit(f"no {LIB} — run from the repo root or pass the root as argv[1]")
declared, _external = declared_modules(LIB)
mods = sorted(m for m in declared if (SRC / m).is_dir())
file_mods = sorted(m for m in declared if not (SRC / m).is_dir())

# Scan tests/ and examples/ too. An earlier version looked only at src/ and reported
# `a2a` as unreferenced; tests/evals.rs pins its wire shape as a release gate. An
# integration test is a consumer, and the one most likely to be the ONLY consumer of a
# protocol surface that has not shipped yet — precisely the case this is meant to find.
files = list(SRC.rglob("*.rs"))
for extra in ("tests", "examples", "benches", "gui", "planner-wasm"):
    d = ROOT / extra
    if d.is_dir():
        files += list(d.rglob("*.rs"))

rows = []
# Pre-compute per-file evidence once, instead of re-reading every file per module.
per_file: list[tuple[Path, set[str], str]] = []
for f in files:
    text = f.read_text(encoding="utf-8", errors="replace")
    per_file.append((f, use_tree_heads(text), text))

for m in mods:
    own = SRC / m
    direct = direct_ref(m)
    loc = sum(sum(1 for _ in f.open(encoding="utf-8", errors="replace"))
              for f in own.rglob("*.rs"))
    ext, in_main = 0, False
    for f, heads, text in per_file:
        try:
            f.relative_to(own)
            continue
        except ValueError:
            pass
        if m in heads or direct.search(text):
            ext += 1
            if f.name == "main.rs":
                in_main = True
    rows.append((m, loc, ext, in_main))

rows.sort(key=lambda r: (r[2], -r[1]))
print("── Directory modules in src/ ──")
print(f"{'module':<16}{'loc':>7}{'ext refs':>10}  main")
print("-" * 44)
for m, loc, ext, in_main in rows:
    print(f"{m:<16}{loc:>7}{ext:>10}  {'yes' if in_main else ''}")

islands = [r for r in rows if r[2] == 0]
print(f"\nzero external references: {len(islands)} module(s), {sum(r[1] for r in islands)} LOC")
print("  " + (", ".join(r[0] for r in islands) if islands else "none"))
print(f"\ntotal: {len(rows)} directory modules, {sum(r[1] for r in rows)} LOC")
if file_mods:
    print(f"single-file modules (not surveyed): {', '.join(file_mods)}")
print("\nReminder: zero external references means 'removing it would not break the")
print("build'. That is where the judgement starts, not where it ends.")


# ── The same question, one level up ─────────────────────────────────────────
#
# Everything above this line is regex evidence about three modules. Everything
# below is cargo's own dependency graph over thirty-five packages, which is not
# a proxy for "would removing it break the build" — it is that fact.


def workspace_packages(root: Path) -> list[dict]:
    """Workspace members, straight from cargo.

    Loud on failure rather than silent: an empty list here would print "0
    crates, no islands" and read exactly like a clean bill. Same rule
    `sync_upstream.undeclared_upstream_files` follows — a check that cannot run
    must not look like a check that ran.
    """
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, cwd=str(root), timeout=180,
        )
    except (OSError, subprocess.SubprocessError) as e:
        sys.exit(f"cargo metadata could not run ({e}) — crate survey not attempted")
    if out.returncode != 0:
        sys.exit(f"cargo metadata failed:\n{out.stderr.strip()}")
    packages = json.loads(out.stdout).get("packages", [])
    if not packages:
        sys.exit("cargo metadata returned no packages — the query is wrong, not the workspace")
    return packages


def is_entry_point(pkg: dict) -> str:
    """Why nothing is expected to depend on this package, or "" if something is."""
    kinds = {k for t in pkg["targets"] for k in t["kind"]}
    if "bin" in kinds:
        return "binary"
    if kinds & {"cdylib", "staticlib"}:
        return "build artifact"
    return ""


def crate_loc(pkg: dict) -> int:
    src = Path(pkg["manifest_path"]).parent / "src"
    if not src.is_dir():
        return 0
    return sum(sum(1 for _ in f.open(encoding="utf-8", errors="replace"))
               for f in src.rglob("*.rs"))


packages = workspace_packages(ROOT)
members = {p["name"] for p in packages}

# name -> who depends on it, split by kind. A crate carried only by another
# crate's dev-dependencies is reachable from the test suite and from nothing
# that ships, which is a different answer and worth not blurring.
#
# That column reads zero for every crate today, and it is a real zero rather
# than a broken parse: the workspace has 420 normal dependency edges and 18
# dev ones, and none of the 18 point at a workspace member. Checked, because a
# column that is always zero looks like a bug and the next person to notice it
# would be right to suspect one. It stays because the day it is non-zero is
# exactly the day someone needs to be told.
normal: dict[str, set[str]] = {n: set() for n in members}
devish: dict[str, set[str]] = {n: set() for n in members}
for pkg in packages:
    for dep in pkg["dependencies"]:
        if dep["name"] not in members or dep["name"] == pkg["name"]:
            continue
        bucket = normal if dep.get("kind") is None else devish
        bucket[dep["name"]].add(pkg["name"])

binaries = {p["name"] for p in packages if "bin" in {k for t in p["targets"] for k in t["kind"]}}

crates = []
for pkg in packages:
    name = pkg["name"]
    crates.append({
        "name": name,
        "loc": crate_loc(pkg),
        "normal": len(normal[name]),
        "dev": len(devish[name]),
        "in_bin": bool(normal[name] & binaries),
        "entry": is_entry_point(pkg),
    })

crates.sort(key=lambda c: (bool(c["entry"]), c["normal"], -c["loc"]))

print("\n\n── Workspace crates ──")
print(f"{'crate':<22}{'loc':>7}{'dependents':>12}{'dev-only':>10}  bin")
print("-" * 55)
for c in crates:
    if c["entry"]:
        print(f"{c['name']:<22}{c['loc']:>7}{'—':>12}{'—':>10}  ({c['entry']})")
    else:
        print(f"{c['name']:<22}{c['loc']:>7}{c['normal']:>12}{c['dev']:>10}"
              f"  {'yes' if c['in_bin'] else ''}")

isles = [c for c in crates if not c["entry"] and c["normal"] == 0]
dev_only = [c for c in isles if c["dev"]]
print(f"\nzero in-workspace dependents: {len(isles)} crate(s), "
      f"{sum(c['loc'] for c in isles)} LOC")
print("  " + (", ".join(c["name"] for c in isles) if isles else "none"))
if dev_only:
    print(f"  of those, reachable from a dev-dependency only: "
          f"{', '.join(c['name'] for c in dev_only)}")
entries = [c for c in crates if c["entry"]]
print(f"\ntotal: {len(crates)} packages, {sum(c['loc'] for c in crates)} LOC; "
      f"{len(entries)} excluded as entry points "
      f"({', '.join(c['name'] for c in entries)})")
print("\nThis half is exact where the half above it is evidence: cargo resolved")
print("these edges to build the workspace. What it still cannot tell you is")
print("whether a crate with dependents is *worth* having — that judgement is")
print("the reason this file is called a survey and not a gate.")
