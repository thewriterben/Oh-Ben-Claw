#!/usr/bin/env python3
"""check_bench_constants.py — the numbers in bench prose are the numbers in the
firmware.

Why this exists
---------------
Bench documents are uniquely expensive to get wrong. Every other stale document
wastes a reader's time; a stale bench document puts someone in front of
hardware with no way to tell a wrong instruction from a broken board. The
failure looks like a dark LED or a sensor that reads nothing, which is exactly
what a working gate and an unfitted sensor also look like.

On 2026-08-21 the firmware's Track 0 allow-list lost GPIO 6 and its I2C bus
moved from 4/5 to 5/6. `docs/HARDWARE-TEST-WALKTHROUGH.md` had been current on
2026-07-30 and was now telling an operator to wire an LED to pin 6 — a pin the
node would refuse — and to expect a boot banner reading `SDA=4, SCL=5` that the
node would never print. Nothing reported either: no gate compares a number in
prose against the constant it describes.

What it checks
--------------
Facts that a bench document states about the firmware, against the firmware:

  * the Track 0 output allow-list, per board,
  * the I2C bus pins, per board,
  * the node id,
  * the `I2C sensor bus ready (SDA=…, SCL=…)` banner the operator is told to
    look for.

The firmware side is read from `board.rs` and `main.rs` rather than restated
here, so this script cannot drift from them either.

What it deliberately does not check
-----------------------------------
* **Prose that is dated or quoted.** A line carrying a date, or inside a
  blockquote, is a record of what was once true — `check_tree.py` uses the same
  rule and for the same reason. Correcting history to match the present is how
  a document stops being trustworthy.

  This cuts both ways, and it bit within an hour of the script being written.
  Annotating a live pin list with "changed on 2026-08-21" on the *same line*
  makes that line a record, and the check goes quiet for exactly the line most
  worth checking. Keep the dates in a note near the table, not in the row.
  There is a selftest case for it.
* **Whether the numbers are right for the board.** That needs a bench. This
  only proves prose and firmware say the same thing; they could still agree and
  both be wrong, which is what the hardware run is for.
* **Any document that is not a bench document.** The list is explicit below.

Run:  python scripts/check_bench_constants.py
      python scripts/check_bench_constants.py --selftest
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FW = ROOT / "firmware" / "obc-esp32-s3" / "src"

# Documents a person reads with hardware in front of them.
BENCH_DOCS = [
    "docs/HARDWARE-TEST-WALKTHROUGH.md",
    "docs/BENCH-WALKTHROUGH.md",
    "docs/BENCH-WALKTHROUGH-ADVANCED.md",
    "docs/BENCH-ACCEPTANCE.md",
    "docs/BENCH-TEST-HARDWARE.md",
    "docs/BENCH-PINOUT-CARDS.md",
    "docs/BENCH-BINDER.md",
    "firmware/obc-esp32-s3/BRINGUP.md",
]

DATED = re.compile(r"\b20\d\d-\d\d-\d\d\b")

# A bolded number run is only an allow-list claim if the line says so.
ALLOWLIST_CONTEXT = re.compile(
    r"allow-list|allowlist|safe output|output pin|output gpio|track 0|track0", re.I
)


def firmware_facts() -> dict[str, object]:
    """Read the constants from the firmware, rather than restating them."""
    board = (FW / "board.rs").read_text(encoding="utf-8")
    main = (FW / "main.rs").read_text(encoding="utf-8")

    def board_body(name: str) -> str:
        m = re.search(rf"pub const {name}: Board = Board \{{(.*?)\n\}};", board, re.S)
        if not m:
            sys.exit(f"could not find `{name}` in board.rs")
        return m.group(1)

    # Shape-aware, not generic: a `([^,]+),` match stops at the first comma,
    # which is inside `&[21, 3, 7, 8]`. That bug shipped in this script's first
    # run and read every pin list as its first element — the selftest caught it
    # by failing the cases that were supposed to pass.
    def output_pins(name: str) -> list[int]:
        m = re.search(r"output_pins: &\[([^\]]*)\]", board_body(name))
        if not m:
            sys.exit(f"could not find `output_pins` on `{name}`")
        return [int(x) for x in re.findall(r"\d+", m.group(1))]

    def i2c(name: str) -> list[int]:
        m = re.search(r"i2c: Some\(\(([^)]*)\)\)", board_body(name))
        if not m:
            sys.exit(f"could not find `i2c` on `{name}`")
        return [int(x) for x in re.findall(r"\d+", m.group(1))]

    node = re.search(r'const NODE_ID: &str = "([^"]+)"', main)
    if not node:
        sys.exit("could not find NODE_ID in main.rs")

    return {
        "xiao_outputs": output_pins("XIAO_ESP32_S3"),
        "xiao_i2c": i2c("XIAO_ESP32_S3"),
        "wave_outputs": output_pins("WAVESHARE_ESP32_S3_TOUCH_LCD_21"),
        "wave_i2c": i2c("WAVESHARE_ESP32_S3_TOUCH_LCD_21"),
        "node_id": node.group(1),
    }


def is_record(line: str) -> bool:
    """A dated line or a blockquote is a record of what was true, not a claim
    about now. Same rule `check_tree.py` uses, for the same reason."""
    return bool(DATED.search(line)) or line.lstrip().startswith(">")


def check_line(line: str, f: dict) -> list[str]:
    """Facts this line asserts about the firmware that the firmware denies."""
    problems: list[str] = []
    if is_record(line):
        return problems

    # The boot banner an operator is told to look for.
    for m in re.finditer(r"I2C sensor bus ready \(SDA=(\d+), SCL=(\d+)\)", line):
        got = [int(m.group(1)), int(m.group(2))]
        if got not in (f["xiao_i2c"], f["wave_i2c"]):
            problems.append(
                f"banner says SDA={got[0]}, SCL={got[1]}; the firmware prints "
                f"{f['xiao_i2c']} (default) or {f['wave_i2c']} (waveshare)"
            )

    # An SDA=/SCL= pair stated as the bus.
    for m in re.finditer(r"SDA\s*=\s*(?:GPIO)?(\d+),\s*SCL\s*=\s*(?:GPIO)?(\d+)", line):
        got = [int(m.group(1)), int(m.group(2))]
        if got not in (f["xiao_i2c"], f["wave_i2c"]):
            problems.append(
                f"names an I2C bus of {got}; the firmware opens "
                f"{f['xiao_i2c']} (default) or {f['wave_i2c']} (waveshare)"
            )

    # A bolded pin list offered as the allow-list.
    #
    # Only when the line says that is what it is. Bolded number runs are common
    # in these documents and usually mean something else entirely: the first
    # version of this check flagged `D0–D7 **11,9,8,10,12,18,17,16**`, the
    # ESP32-S3-EYE's camera data bus, as a Track 0 allow-list. A gate that
    # cannot tell those apart gets switched off.
    if ALLOWLIST_CONTEXT.search(line):
        for m in re.finditer(r"\*\*((?:\d+\s*,\s*)+\d+)\*\*", line):
            got = [int(x) for x in re.findall(r"\d+", m.group(1))]
            if got not in (f["xiao_outputs"], f["wave_outputs"]):
                problems.append(
                    f"offers pins {got} as an allow-list; the firmware allows "
                    f"{f['xiao_outputs']} (default) or {f['wave_outputs']} (waveshare)"
                )

    # A node id in backticks.
    for m in re.finditer(r"`(obc-esp32-s3-[\w-]+|bench-\d+)`", line):
        if m.group(1) != f["node_id"] and m.group(1).startswith("obc-esp32-s3"):
            problems.append(
                f"names node `{m.group(1)}`; NODE_ID is `{f['node_id']}`"
            )
    return problems


SELFTEST = [
    ("the allow-list that went stale today",
     "wire the LED to an allow-listed pin — default build: **21, 3, 6, 7, 8**", False),
    ("the corrected allow-list",
     "wire the LED to an allow-listed pin — default build: **21, 3, 7, 8**", True),
    ("the waveshare allow-list is also accepted",
     "on that build the safe outputs are **43, 44**", True),
    ("a bolded pin run that is not an allow-list",
     "OV2640: XCLK **15** · D0–D7 **11,9,8,10,12,18,17,16** · VSYNC **6**", True),
    ("the banner that went stale today",
     "I2C sensor bus ready (SDA=4, SCL=5)", False),
    ("the corrected banner",
     "I2C sensor bus ready (SDA=5, SCL=6)", True),
    ("an I2C bus stated in prose",
     "Default: SDA=GPIO4, SCL=GPIO5 · Waveshare: SDA=15, SCL=7.", False),
    ("the corrected bus",
     "Default: SDA=GPIO5, SCL=GPIO6 · Waveshare: SDA=15, SCL=7.", True),
    ("a wrong node id",
     "the banner must read `obc-esp32-s3-999`", False),
    ("the right node id",
     "the banner must read `obc-esp32-s3-001`", True),
    ("a dated line is a record, not a claim",
     "It said **21, 3, 6, 7, 8** until 2026-08-21, when GPIO 6 left the list.", True),
    ("...which is also how a live claim gets silenced by mistake",
     "safe outputs **21, 3, 6, 7, 8** (GPIO 6 leaves this list on 2026-08-21)", True),
    ("a blockquote is a record too",
     "> the old procedure used **21, 3, 6, 7, 8** and a bus on SDA=4, SCL=5", True),
    ("prose with no firmware constants in it",
     "Attach the antenna before powering the board.", True),
]


def selftest(f: dict) -> int:
    failed = []
    for label, line, want_pass in SELFTEST:
        got_pass = not check_line(line, f)
        if got_pass != want_pass:
            failed.append(f"{label}: expected {'pass' if want_pass else 'FAIL'}, got the other")
    print(f"selftest: {len(SELFTEST) - len(failed)}/{len(SELFTEST)} cases behave as stated")
    for line in failed:
        print("  x " + line)
    if failed:
        return 1
    print("ok: the gate tells a live claim from a dated record, and both boards' "
          "constants from a wrong one")
    return 0


def main(argv: list[str]) -> int:
    f = firmware_facts()
    if "--selftest" in argv:
        return selftest(f)

    problems: list[str] = []
    scanned = 0
    for rel in BENCH_DOCS:
        path = ROOT / rel
        if not path.exists():
            problems.append(f"{rel}: listed as a bench document and not present")
            continue
        scanned += 1
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for p in check_line(line, f):
                problems.append(f"{rel}:{n}: {p}")

    print(
        f"checked {scanned} bench document(s) against the firmware: "
        f"outputs {f['xiao_outputs']}/{f['wave_outputs']}, "
        f"i2c {f['xiao_i2c']}/{f['wave_i2c']}, node `{f['node_id']}`"
    )
    if problems:
        print("\nbench prose the firmware contradicts:")
        for p in problems:
            print("  x " + p)
        print(
            "\nThese are read by someone standing at a bench, where a wrong\n"
            "instruction and a broken board look identical."
        )
        return 1
    print("ok: every firmware constant a bench document states matches the firmware")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
