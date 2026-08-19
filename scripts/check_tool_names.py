"""Tool names in shipped configs must be tool names the agent actually has.

Why this exists
---------------
`examples/config-cutting-edge.toml` shipped this policy:

    [[security.policies]]
    name         = "deny-etc"
    tool_pattern = "file_write"
    arg_contains = "/etc/"
    action       = "deny"

`tool_pattern` is glob-matched against a tool's `name()`. No tool is called
`file_write` — the file tool is called `file`, and read/write/append/delete are
values of its `action` argument. So the policy matched nothing, and a reader
copying it believed writes to `/etc/` were denied.

The same file listed `file_write` in `autonomy.always_ask`, and
`config-nanopi-deployment.toml` granted two orchestrator agents a tool called
`memory_note` (the tool is `memory`).

This is a recurrence, not a discovery. The Accelerapp alignment already found
the orchestrator being handed `file_read`, `file_write`, `http_get` and
`memory_note` against real tools named `file`, `http` and `memory`. That was
fixed in the code and not in the configs, because nothing had ever compared the
two.

What counts as a real name
--------------------------
The union of three vocabularies, because a tool can be real by three routes:

  * `fn name(&self) -> &str { "..." }` anywhere in the workspace — the builtin
    tools, and the peripheral tools in obc-peripherals that only exist once a
    node announces itself;
  * `VALID_CAPABILITIES` in the peripherals registry — what a node may announce;
  * names obc-approval classifies risk for, which is something knowing a name.

The first pass here scanned obc-tools only and called `gpio_write` unknown. It
is a real peripheral tool. A name checker that does not know where names come
from invents violations, which is worse than not checking.

Limits
------
Only exact names are checked. A `tool_pattern` containing a glob character is
skipped: `browser_*` is a legitimate pattern over seven real tools, and this
script is not a glob evaluator. That is the gap through which a wrong pattern
could still pass, and it is narrower than the one that let `file_write` through.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from console import use_utf8_stdout  # noqa: E402

use_utf8_stdout()

ROOT = Path(__file__).resolve().parent.parent
NAME_RE = re.compile(r'fn name\(&self\)\s*->\s*&(?:\'\w+\s+)?str\s*\{\s*"([^"]+)"')


def tool_names() -> dict[str, str]:
    """Every `Tool::name()` literal in the workspace, mapped to where it lives."""
    out: dict[str, str] = {}
    roots = [ROOT / "src"] + sorted((ROOT / "crates").glob("*/src"))
    for r in roots:
        if not r.is_dir():
            continue
        for f in r.rglob("*.rs"):
            text = f.read_text(encoding="utf-8", errors="replace")
            for m in NAME_RE.finditer(text):
                out.setdefault(m.group(1), f.relative_to(ROOT).as_posix())
    return out


def capabilities() -> set[str]:
    reg = ROOT / "crates/obc-planner/src/peripherals/registry.rs"
    if not reg.is_file():
        return set()
    m = re.search(r"VALID_CAPABILITIES[^=]*=\s*&?\[(.*?)\];",
                  reg.read_text(encoding="utf-8", errors="replace"), re.S)
    return set(re.findall(r'"([^"]+)"', m.group(1))) if m else set()


def risk_classified() -> set[str]:
    p = ROOT / "crates/obc-approval/src/lib.rs"
    if not p.is_file():
        return set()
    return set(re.findall(r'"([a-z][a-z0-9_]+)"\s*=>',
                          p.read_text(encoding="utf-8", errors="replace")))


def shipped_configs() -> list[Path]:
    out = [ROOT / "config.example.toml"]
    out += sorted((ROOT / "examples").glob("*.toml"))
    return [p for p in out if p.is_file()]


def referenced(data: dict) -> dict[str, list[str]]:
    """Every place a shipped config names a tool."""
    refs: dict[str, list[str]] = {}

    def add(name: str, where: str) -> None:
        if isinstance(name, str) and name:
            refs.setdefault(name, []).append(where)

    aut = data.get("autonomy", {})
    for key in ("auto_approve", "always_ask"):
        for n in aut.get(key, []) or []:
            add(n, f"autonomy.{key}")
    for pol in data.get("security", {}).get("policies", []) or []:
        pat = pol.get("tool_pattern")
        # A glob is a legitimate pattern over several real tools; this script
        # does not evaluate globs and says so rather than guessing.
        if isinstance(pat, str) and not any(c in pat for c in "*?"):
            add(pat, f"security.policies[{pol.get('name')}].tool_pattern")
    for agent in data.get("orchestrator", {}).get("agents", []) or []:
        for n in agent.get("tools", []) or []:
            add(n, f"orchestrator.agents[{agent.get('name')}].tools")
    return refs


def main() -> int:
    names = tool_names()
    caps = capabilities()
    risk = risk_classified()
    known = set(names) | caps | risk

    # A vocabulary that failed to load would clear every config in the tree,
    # and the clean bill would read exactly like a real one.
    if len(names) < 20 or not caps:
        print(f"!! vocabulary looks wrong: {len(names)} tool name(s), "
              f"{len(caps)} capabilities — refusing to report a clean bill",
              file=sys.stderr)
        return 2

    configs = shipped_configs()
    if len(configs) < 2:
        print(f"!! found only {len(configs)} shipped config(s) — the discovery "
              f"glob is wrong, not the tree", file=sys.stderr)
        return 2

    print(f"vocabulary: {len(names)} tool name(s), {len(caps)} capabilities, "
          f"{len(risk)} risk-classified — {len(known)} distinct")

    problems: list[tuple[str, str, list[str]]] = []
    checked = 0
    for path in configs:
        try:
            with path.open("rb") as fh:
                data = tomllib.load(fh)
        except tomllib.TOMLDecodeError as e:
            print(f"!! {path.name} does not parse: {e}", file=sys.stderr)
            return 2
        refs = referenced(data)
        checked += len(refs)
        for name, where in sorted(refs.items()):
            if name not in known:
                problems.append((path.name, name, sorted(set(where))))

    print(f"{checked} tool reference(s) across {len(configs)} shipped config(s)\n")
    if not problems:
        print("ok: every tool named in a shipped config is a tool this agent has")
        return 0

    print("── Named in a config, and not a tool ──")
    for cfg, name, where in problems:
        print(f"  {cfg}")
        print(f"    {name}")
        for w in where:
            print(f"      at {w}")
    print(f"\n{len(problems)} unknown tool name(s).")
    print("A safety list or policy naming a tool that does not exist matches")
    print("nothing, and reads to whoever wrote it as though it matches something.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
