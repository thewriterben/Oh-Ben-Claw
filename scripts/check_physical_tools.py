"""A tool that actuates must declare it, because the Track 0 gate believes it.

The bug this exists to prevent
------------------------------
`track0_authorize` in obc-agent opens with:

    if !risk.physical {
        return Ok(());
    }

Not "skip the limit check" -- return. Before the `SafetyGate` check on
`(node_id, tool, pin, value)`, and before the tamper-evident audit record. The
function pulls `pin` and `value` out of the arguments, so it was written for
`gpio_write` and calls shaped like it.

`Tool::risk_class` defaults to `RiskClass::safe()`, non-physical, and its doc
comment says tools that actuate the real world "MUST override this so the
approval layer and safety gate treat them accordingly".

Nothing enforced that MUST. On 2026-08-18 every tool in obc-peripherals took the
default -- `gpio_write`, `pwm_control`, `i2c_write`, `spi_transfer`,
`stm32_flash`, `rpi_camera_capture` -- and so did `MqttNodeTool` and
`P2pNodeTool`, which is how *every* tool announced by *every* peripheral node
reaches the agent. The gate and the audit chain were skipped for the entire
class of call they were built for, and every other check stayed green because
nothing else reads `risk_class`.

Why source-level rather than a test
-----------------------------------
The peripheral tool set is `cfg`-gated: bus tools are Linux-only, board drivers
are behind features. A runtime test would enumerate a different set on every
platform and would have to be loose enough to pass on all of them, which is
loose enough to miss the thing. The property being checked is a property of the
declaration, so it is checked where the declaration is.

Limits
------
Name-based, and deliberately so: the name is the part a reviewer reads. A tool
called `*_write` that reports `physical: false` is either misnamed or
misclassified, and both should stop a build. It cannot see a tool that actuates
under a name that does not say so -- `siren`, `unlock`, `dispense` -- which is
the gap, and the answer to it is naming, not a longer regex.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402

use_utf8_stdout()

ROOT = Path(__file__).resolve().parent.parent

IMPL_RE = re.compile(r"impl Tool for (\w+)\s*\{")
NAME_RE = re.compile(r'fn name\(&self\)\s*->\s*&(?:\'\w+\s+)?str\s*\{\s*"([^"]+)"')
RISK_RE = re.compile(r"fn risk_class\(&self\)")
PHYSICAL_RE = re.compile(r"RiskClass::physical\s*\(")

ACTUATES = (
    lambda n: n.endswith("_write"),
    lambda n: n.endswith("_flash"),
    lambda n: n.endswith("_reset"),
    lambda n: n.endswith("_capture"),
    lambda n: n.startswith("pwm_"),
    lambda n: n == "spi_transfer",
    lambda n: n == "move_actuator",
)


def actuates(name: str) -> bool:
    return any(test(name) for test in ACTUATES)


def impl_blocks(text: str):
    """(struct, tool_name, body) for every `impl Tool for X` in a file."""
    for m in IMPL_RE.finditer(text):
        depth, i = 0, m.end() - 1
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        body = text[m.end():i]
        nm = NAME_RE.search(body)
        if nm:
            yield m.group(1), nm.group(1), body


def main() -> int:
    roots = [ROOT / "src"] + sorted((ROOT / "crates").glob("*/src"))
    files = [f for r in roots if r.is_dir() for f in r.rglob("*.rs")]

    impls, undeclared, unphysical = 0, [], []
    for f in files:
        text = f.read_text(encoding="utf-8", errors="replace")
        if "impl Tool for" not in text:
            continue
        rel = f.relative_to(ROOT).as_posix()
        for struct, name, body in impl_blocks(text):
            impls += 1
            if not actuates(name):
                continue
            if not RISK_RE.search(body):
                undeclared.append((rel, struct, name))
            elif not PHYSICAL_RE.search(body):
                unphysical.append((rel, struct, name))

    if impls < 30:
        print(f"!! found only {impls} `impl Tool` block(s) — the scan is wrong, "
              f"not the tree", file=sys.stderr)
        return 2

    print(f"{impls} tool implementation(s) scanned")

    problems = undeclared + unphysical
    if not problems:
        print("ok: every tool whose name says it actuates declares itself physical")
        return 0

    print("\n── Actuates by name, and the safety gate will not see it ──")
    for rel, struct, name in undeclared:
        print(f"  {name:<22} {struct} ({rel})")
        print(f"{'':<24}no risk_class — inherits RiskClass::safe()")
    for rel, struct, name in unphysical:
        print(f"  {name:<22} {struct} ({rel})")
        print(f"{'':<24}declares risk_class, but not as physical")
    print(f"\n{len(problems)} tool(s). `track0_authorize` returns before the "
          f"SafetyGate\ncheck and before the audit record when `physical` is "
          f"false, so these calls\nreach the hardware ungated and unlogged.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
