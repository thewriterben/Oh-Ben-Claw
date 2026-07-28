"""Gate 3, last piece: remove the config section for a subsystem that no longer exists.

`src/memory/personality.rs` is gone — a documented SOUL.md/USER.md store that
nothing ever called. The config struct outlived it: `[personality]` still parsed,
still had defaults, still had two tests asserting those defaults. That is worse
than either shipping the feature or not having it, because the config file is
the surface a user actually reads. A key that parses cleanly and does nothing is
a promise the code does not keep.

Removing it is safe for existing config files: the root `Config` does not
`deny_unknown_fields`, so a leftover `[personality]` block is ignored rather than
becoming a hard parse error on upgrade. Ignoring it is exactly what the code did
before, minus the pretence.
"""

import pathlib
import re
import sys

p = pathlib.Path(sys.argv[1]) / "src/config/mod.rs"
with open(p, encoding="utf-8", newline="") as fh:
    s = fh.read()
before = len(s)

# 1. The struct, its doc comment, and the section banner above it.
struct_block = re.search(
    r"// ── Personality Configuration \(new in Phase 11\) ─+\r?\n"
    r"(?:.*?\r?\n)*?"
    r"pub struct PersonalityConfig \{(?:.*?\r?\n)*?\}\r?\n\r?\n",
    s,
)
if not struct_block:
    sys.exit("struct block not matched — refusing to guess")
s = s[: struct_block.start()] + s[struct_block.end() :]

# 2. The field on Config.
field = (
    "    /// Personality file configuration — SOUL.md and USER.md (new in Phase 11).\r\n"
    "    #[serde(default)]\r\n"
    "    pub personality: PersonalityConfig,\r\n"
)
if field not in s:
    field = field.replace("\r\n", "\n")
if field not in s:
    sys.exit("Config field not matched — refusing to guess")
s = s.replace(field, "")

# 3. The test asserting defaults on a struct that no longer exists.
test = re.search(
    r"    #\[test\]\r?\n"
    r"    fn personality_config_default_is_empty\(\) \{(?:.*?\r?\n)*?    \}\r?\n\r?\n",
    s,
)
if not test:
    sys.exit("test not matched — refusing to guess")
s = s[: test.start()] + s[test.end() :]

# 4. The shared proxy/personality test keeps its proxy half, renamed to say what
#    it still checks. A test named for two things that checks one is a lie the
#    next reader has to discover.
s = s.replace(
    "    fn root_config_has_proxy_and_personality_fields() {",
    "    fn root_config_has_proxy_field() {",
)
s = re.sub(r"\r?\n        assert!\(config\.personality\.soul_path\.is_none\(\)\);", "", s)

with open(p, "w", encoding="utf-8", newline="") as fh:
    fh.write(s)
print(f"src/config/mod.rs: {before} -> {len(s)} bytes")
leftover = [
    f"{i}: {ln.strip()}"
    for i, ln in enumerate(s.splitlines(), 1)
    if "personality" in ln.lower() or "PersonalityConfig" in ln
]
print("leftover references:", leftover or "none")
