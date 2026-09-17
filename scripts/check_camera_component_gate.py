#!/usr/bin/env python3
"""`main` must not carry an uncommented esp32-camera component block.

    python scripts/check_camera_component_gate.py            # report only
    python scripts/check_camera_component_gate.py --enforce  # exit 1 if uncommented

# Why

`[[package.metadata.esp-idf-sys.extra_components]]` in
`firmware/obc-esp32-s3/Cargo.toml` pulls the esp32-camera IDF component into
**every** build of that crate. It cannot be gated on a cargo feature: the option
is an exclude in esp-idf-sys's `try_from_env()`, the `cargo metadata` call that
reads it is passed no feature flags, and `CARGO_FEATURE_*` is read nowhere in
that crate's build script. Checked in esp-idf-sys 0.37.2's source on 2026-09-16,
not assumed.

So with it uncommented, a *default* build produces a different binary from the
one on the live mesh node — PSRAM on, a camera component compiled in, a
different flash layout. Flashing that to `obc-esp32-s3-001` is the failure this
guards against.

# Why a script and not a rule

The original containment was: keep the block commented on `main`, uncomment it
only on a `camera-bringup` branch, "a branch name you can see in your prompt".
That failed twice on the day it was written:

  1. I worked for an hour on `main` believing I was on the branch, and only
     noticed when a build failed for an unrelated-looking reason.
  2. A shared `CARGO_TARGET_DIR` meant building on `main` silently destroyed the
     camera bindings the branch depended on — esp-idf-sys's build script does not
     re-run on `Cargo.toml` metadata changes, so coming back to the branch did
     not restore them.

Neither is a discipline problem that more discipline fixes. A prompt protects a
human at a terminal and protects nothing else. This is the comparison a machine
can make.

# Enforcement

`--enforce` is passed by CI only on `main`, because the bring-up branch is
*supposed* to have it uncommented. A check that is permanently red on a working
branch teaches people to ignore red, which is worse than no check.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "firmware" / "obc-esp32-s3" / "Cargo.toml"

# A line that starts the block with no leading `#`.
LIVE = re.compile(r"^\s*\[\[package\.metadata\.esp-idf-sys\.extra_components\]\]", re.M)
# The same line commented out, which is the state `main` should be in.
COMMENTED = re.compile(r"^\s*#\s*\[\[package\.metadata\.esp-idf-sys\.extra_components\]\]", re.M)


def main() -> int:
    enforce = "--enforce" in sys.argv
    if not MANIFEST.is_file():
        print(f"not found: {MANIFEST}")
        return 2

    text = MANIFEST.read_text(encoding="utf-8")
    live = LIVE.search(text)
    commented = COMMENTED.search(text)

    if not live and not commented:
        # Neither form present: the block was deleted or renamed. That is a
        # change this check can no longer reason about, so it says so rather
        # than passing silently.
        print(
            "neither a live nor a commented `extra_components` block was found in\n"
            f"  {MANIFEST.relative_to(ROOT)}\n"
            "This check can no longer tell which state the tree is in. Update it."
        )
        return 2

    if live:
        line = text[: live.start()].count("\n") + 1
        print(
            f"  UNCOMMENTED at {MANIFEST.relative_to(ROOT)}:{line}\n"
            "  A default build from this tree pulls in the esp32-camera component and\n"
            "  produces a different binary from the one on the live mesh node.\n"
            "  Expected on a camera bring-up branch; never on main."
        )
        if enforce:
            print("\n  FAILED: this must not be on main. Re-comment the four lines.")
            return 1
        print("\n  (not enforcing: pass --enforce on main)")
        return 0

    print(f"  ok: the component block is commented out in {MANIFEST.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
