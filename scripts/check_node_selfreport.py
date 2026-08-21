#!/usr/bin/env python3
"""check_node_selfreport.py — what the node says about itself comes from the
same constants as what it does.

Why this exists
---------------
The ESP32-S3 firmware answers `capabilities` with a JSON object describing the
board, its safe GPIO outputs, its I2C bus, and whether it has a camera and a
microphone. A host that has never seen the board has no other way to know any
of it.

Every one of those fields was a hardcoded literal, and every one of them is
wrong on some build:

  * `"board": "seeed-xiao-esp32-s3"` — announced by the Waveshare build too.
  * `"gpio": [21, 3, 6, 7, 8]` — the XIAO's `OUTPUT_PINS`. The Waveshare build
    allows `[43, 44]`. A host reading this believes it may drive five pins the
    gate will refuse, and does not know about the two it will accept.
  * `"i2c_bus": [4, 5]` — the Waveshare build opens 15/7.
  * `"microphone": true` — the I2S mic is `#[cfg(not(board-waveshare-21))]`.
    That build has no mic and says it has one.
  * `"camera": false` — a `--features camera` build has one and says it does
    not.

This is the same shape as every other thing found in this tree this month: a
measurement that counts the report about itself converges on a lie. The node
was not measuring anything. It was reciting a constant that happened to be
true for one build.

What it checks
--------------
That no build-varying field in the `capabilities` object is a literal. Each
must be an expression that the compiler ties to the behaviour — `OUTPUT_PINS`,
`BOARD_NAME`, `I2C_PINS`, `cfg!(...)`. Once a field references the constant the
code acts on, the two cannot disagree, and this gate has nothing left to prove
per-value.

It is a lint, not a proof of the values. The values are the compiler's job
after the fields are wired to the constants; keeping them wired is this
script's job, and it needs no ESP toolchain to do it — which matters, because
nothing in CI can build this crate.

What it deliberately does not check
-----------------------------------
* **That the pin *numbers* are right for the board.** `I2C_PINS = (4, 5)` and
  `pins.gpio4, pins.gpio5` are two facts that must agree and nothing here can
  tie them; a const cannot be compared against a HAL pin type. Named in the
  firmware comment rather than pretended away.
* **The static fields.** `node_id`, `firmware_version` and the tool list do
  not vary by build.

Run:  python scripts/check_node_selfreport.py
      python scripts/check_node_selfreport.py --selftest
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIRMWARE = ROOT / "firmware" / "obc-esp32-s3" / "src" / "main.rs"

# Fields whose correct value depends on which board or feature set was built.
# Each maps to what it must be derived from, for the error message.
BUILD_VARYING = {
    "board": "a BOARD_NAME const behind the same cfg as the pin maps",
    "gpio": "OUTPUT_PINS",
    "i2c_bus": "an I2C_PINS const, the one the driver is opened with",
    "camera": "cfg!(feature = \"camera\")",
    "microphone": "cfg!(not(feature = \"board-waveshare-21\"))",
}

# A literal is a bare string, number, array of numbers, or bool. Anything else
# -- an identifier, a macro call, an index -- is an expression the compiler can
# tie to behaviour, which is the whole point.
LITERAL = re.compile(
    r"""^\s*(
          "[^"]*"                # "seeed-xiao-esp32-s3"
        | true | false           # microphone / camera
        | -?\d+(\.\d+)?          # a bare number
        | \[\s*-?\d+(\s*,\s*-?\d+)*\s*,?\s*\]   # [21, 3, 6, 7, 8]
    )\s*$""",
    re.X,
)


def capabilities_block(text: str) -> str:
    """The body of the `capabilities` json! object.

    Anchored on the arm rather than on a line number, so moving the function
    does not silently stop this from checking anything -- a checker that
    quietly finds nothing to check is worse than no checker.
    """
    start = text.find('"capabilities"')
    if start == -1:
        sys.exit("could not find the `capabilities` arm in " + str(FIRMWARE))
    open_brace = text.find("json!({", start)
    if open_brace == -1:
        sys.exit("found the `capabilities` arm but no `json!({` in it")
    i = text.index("{", open_brace + len("json!("))
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i + 1 : j]
    sys.exit("the `capabilities` json! object is not brace-balanced")


def field_values(block: str) -> dict[str, str]:
    """Map top-level `"key": value` pairs to their value text.

    Only depth 0, so the `tools` array's own `"name":`/`"description":` keys do
    not register as capability fields.
    """
    out: dict[str, str] = {}
    depth = 0
    key: str | None = None
    start = 0
    for i, ch in enumerate(block):
        if ch in "[{":
            depth += 1
        elif ch in "]}":
            depth -= 1
        elif ch == ":" and depth == 0:
            m = re.search(r'"([^"]+)"\s*$', block[start:i])
            key = m.group(1) if m else None
            start = i + 1
        elif ch == "," and depth == 0:
            if key:
                out[key] = block[start:i].strip()
            key, start = None, i + 1
    if key:
        out[key] = block[start:].strip()
    return out


# (name, block body, should_pass). The first case is the bug this was built
# for; the second is the same field wired to the constant. If those two ever
# agree, the gate has stopped discriminating.
SELFTEST = [
    ("the hardcoded gpio list that caused this",
     '"node_id": NODE_ID, "gpio": [21, 3, 6, 7, 8], "wifi": true', False),
    ("the same field, wired to OUTPUT_PINS",
     '"node_id": NODE_ID, "gpio": OUTPUT_PINS, "wifi": true', True),
    ("a hardcoded board name",
     '"board": "seeed-xiao-esp32-s3"', False),
    ("a board name behind a const",
     '"board": BOARD_NAME', True),
    ("a hardcoded microphone claim",
     '"microphone": true', False),
    ("a microphone claim from cfg!",
     '"microphone": cfg!(not(feature = "board-waveshare-21"))', True),
    ("a hardcoded i2c bus",
     '"i2c_bus": [4, 5]', False),
    ("an i2c bus from the const the driver uses",
     '"i2c_bus": [I2C_PINS.0, I2C_PINS.1]', True),
    ("fields that do not vary by build are left alone",
     '"node_id": NODE_ID, "firmware_version": FIRMWARE_VERSION, "edge_agent": true', True),
    ("the tools array does not register as a field",
     '"tools": [{"name": "gpio_read", "description": "Read a pin."}]', True),
]


def offenders(block: str) -> list[str]:
    found = []
    for key, want in BUILD_VARYING.items():
        if key not in (fields := field_values(block)):
            continue
        value = fields[key]
        if LITERAL.match(value):
            found.append(
                f'"{key}": {value} is a literal. It must come from {want}, '
                f"or it is only true for one build"
            )
    return found


def selftest() -> int:
    failed = []
    for label, block, want_pass in SELFTEST:
        got_pass = not offenders(block)
        if got_pass != want_pass:
            failed.append(f"{label}: expected {'pass' if want_pass else 'FAIL'}, got the other")
    print(f"selftest: {len(SELFTEST) - len(failed)}/{len(SELFTEST)} cases behave as stated")
    for line in failed:
        print("  x " + line)
    if failed:
        return 1
    print("ok: the gate tells a hardcoded self-report from a derived one")
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()

    if not FIRMWARE.exists():
        sys.exit(f"firmware not found at {FIRMWARE}")
    block = capabilities_block(FIRMWARE.read_text(encoding="utf-8"))
    fields = field_values(block)
    checked = [k for k in BUILD_VARYING if k in fields]

    print(
        f"checked {len(checked)} build-varying field(s) in the node's "
        f"`capabilities` self-report: {', '.join(checked)}"
    )
    bad = offenders(block)
    if bad:
        print("\nfields that describe one build and are announced by every build:")
        for entry in bad:
            print("  x " + entry)
        print(
            "\nA host has no other way to learn any of this. A node that\n"
            "announces pins its own gate will refuse is not describing itself."
        )
        return 1
    print("ok: every build-varying field is tied to the constant the behaviour uses")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
