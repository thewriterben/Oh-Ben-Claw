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
`stm32_flash`, `rpi_camera_capture` -- and so did `MqttNodeTool`, `P2pNodeTool`
and `McpRemoteTool`, which is how every tool announced by a peripheral node, a
P2P peer or a remote MCP server reaches the agent. The gate and the audit chain
were skipped for the entire class of call they were built for, and every other
check stayed green because nothing else reads `risk_class`.

Two rules, and the second one caught what the first could not
-------------------------------------------------------------
1. A tool whose **literal name** says it actuates -- `*_write`, `*_flash`,
   `*_reset`, `*_capture`, `pwm_*`, `spi_transfer`, `move_actuator` -- must
   declare `risk_class`, and must declare it physical.

2. A tool whose name is **not a literal** -- computed at runtime, as
   `MqttNodeTool`, `P2pNodeTool` and `McpRemoteTool` compute theirs from a
   remote announcement -- must declare `risk_class` at all, whatever it says.

Rule 1 shipped first and was blind to the three tools that motivated writing it:
a name-matching heuristic has no name to match. The two node tools were fixed by
hand in the same commit and so looked covered; `McpRemoteTool` was not, and
stayed unclassified until rule 2 existed to see it. A tool whose name is decided
by whoever is on the other end of a socket is exactly the tool a reviewer cannot
classify by reading, so the declaration is the only place the answer can be.

3. Every defaulted method on `trait Tool` must be forwarded by the blanket
   `impl Tool for Arc<dyn Tool>`.

The agent's registry stores `Arc<dyn Tool>`, and `Box::new(Arc::clone(&tool))`
is what it hands the provider per call -- so every one of these declarations is
read *through* that blanket impl. A method the impl forgets to forward does not
fail to compile; it silently returns the trait default, which is precisely the
`RiskClass::safe()` that started all of this, reintroduced one indirection away
from where anyone would look. All nine are forwarded today. Nothing but this
kept them that way.

Why source-level rather than a test
-----------------------------------
The peripheral tool set is `cfg`-gated: bus tools are Linux-only, board drivers
are behind features. A runtime test would enumerate a different set on every
platform and would have to be loose enough to pass on all of them, which is
loose enough to miss the thing. The property being checked is a property of the
declaration, so it is checked where the declaration is.

Limits
------
Rule 1 is name-based, and deliberately so: the name is the part a reviewer
reads. A tool called `*_write` that reports `physical: false` is either misnamed
or misclassified, and both should stop a build. It cannot see a tool that
actuates under a name that does not say so -- `siren`, `unlock`, `dispense` --
which is the gap, and the answer to it is naming, not a longer regex.

Neither rule can tell a correct `physical` from an incorrect one. This checks
that the judgement was made, not that it was right.
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
NAME_FN_RE = re.compile(r"fn name\(&self\)\s*->\s*&(?:'\w+\s+)?str\s*\{")
NAME_LIT_RE = re.compile(r'fn name\(&self\)\s*->\s*&(?:\'\w+\s+)?str\s*\{\s*"([^"]+)"')
RISK_RE = re.compile(r"fn risk_class\(&self\)")
PHYSICAL_RE = re.compile(r"RiskClass::physical\s*\(")

# Below this the scan is broken, not the tree.
MIN_IMPLS = 30

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


TRAIT_RE = re.compile(r"pub trait Tool:[^{]*\{")
ARC_IMPL_RE = re.compile(r"impl Tool for std::sync::Arc<dyn Tool>\s*\{")
FN_RE = re.compile(r"\bfn (\w+)\(&self")


def _brace_body(text: str, start: int) -> str:
    """The `{...}` body beginning at the brace on/after `start`."""
    depth, i = 0, text.index("{", start)
    while i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                break
        i += 1
    return text[text.index("{", start) + 1:i]


def forwarding_gaps(api: Path) -> list[str] | None:
    """Trait methods the `Arc<dyn Tool>` blanket impl does not forward.

    `None` if the two blocks could not be located at all -- a structural change
    to obc-tool-api that this check must report rather than pass through.
    """
    text = api.read_text(encoding="utf-8", errors="replace")
    t, a = TRAIT_RE.search(text), ARC_IMPL_RE.search(text)
    if not t or not a:
        return None
    declared = set(FN_RE.findall(_brace_body(text, t.start())))
    forwarded = set(FN_RE.findall(_brace_body(text, a.start())))
    return sorted(declared - forwarded)


def impl_blocks(text: str):
    """(struct, body) for every `impl Tool for X` in a file, brace-matched."""
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
        yield m.group(1), text[m.end():i]


def main() -> int:
    roots = [ROOT / "src"] + sorted((ROOT / "crates").glob("*/src"))
    files = [f for r in roots if r.is_dir() for f in sorted(r.rglob("*.rs"))]

    impls = 0
    dynamic = 0
    undeclared: list[tuple[str, str, str]] = []
    unphysical: list[tuple[str, str, str]] = []
    unjudgeable: list[tuple[str, str]] = []

    for f in files:
        text = f.read_text(encoding="utf-8", errors="replace")
        if "impl Tool for" not in text:
            continue
        rel = f.relative_to(ROOT).as_posix()
        for struct, body in impl_blocks(text):
            if not NAME_FN_RE.search(body):
                continue  # not the Tool trait we mean
            impls += 1
            declares = bool(RISK_RE.search(body))
            lit = NAME_LIT_RE.search(body)
            if not lit:
                dynamic += 1
                if not declares:
                    unjudgeable.append((rel, struct))
                continue
            name = lit.group(1)
            if not actuates(name):
                continue
            if not declares:
                undeclared.append((rel, struct, name))
            elif not PHYSICAL_RE.search(body):
                unphysical.append((rel, struct, name))

    if impls < MIN_IMPLS:
        print(f"!! found only {impls} `impl Tool` block(s) — the scan is wrong, "
              f"not the tree", file=sys.stderr)
        return 2

    api = ROOT / "crates" / "obc-tool-api" / "src" / "lib.rs"
    gaps = forwarding_gaps(api) if api.is_file() else []
    if gaps is None:
        print("!! could not locate `pub trait Tool` and the `Arc<dyn Tool>` "
              "blanket impl in\n   obc-tool-api — the check is wrong, or the "
              "contract moved", file=sys.stderr)
        return 2

    print(f"{impls} tool implementation(s) scanned "
          f"({dynamic} name themselves at runtime)")

    problems = len(undeclared) + len(unphysical) + len(unjudgeable) + len(gaps)
    if not problems:
        print("ok: every tool that says it actuates declares itself physical, "
              "every tool\n    that cannot say declares something, and "
              "`Arc<dyn Tool>` forwards all of it")
        return 0

    if gaps:
        print("\n── Declared on `Tool`, not forwarded by `Arc<dyn Tool>` ──")
        for fn in gaps:
            print(f"  {fn}()")
            print(f"{'':<4}the registry stores `Arc<dyn Tool>`, so this reads "
                  f"the trait default\n{'':<4}for every tool, and compiles")

    if undeclared or unphysical:
        print("\n── Actuates by name, and the safety gate will not see it ──")
        for rel, struct, name in undeclared:
            print(f"  {name:<22} {struct} ({rel})")
            print(f"{'':<24}no risk_class — inherits RiskClass::safe()")
        for rel, struct, name in unphysical:
            print(f"  {name:<22} {struct} ({rel})")
            print(f"{'':<24}declares risk_class, but not as physical")

    if unjudgeable:
        print("\n── Named at runtime, so nothing here can classify it ──")
        for rel, struct in unjudgeable:
            print(f"  {struct} ({rel})")
            print(f"{'':<24}no literal name and no risk_class: this tool's risk "
                  f"is\n{'':<24}decided by whoever announced it")

    print(f"\n{problems} problem(s). `track0_authorize` returns before the "
          f"SafetyGate\ncheck and before the audit record when `physical` is "
          f"false, so a tool that\nreads as safe — because it said so, or "
          f"because nothing carried what it said —\nreaches the hardware "
          f"ungated and unlogged.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
