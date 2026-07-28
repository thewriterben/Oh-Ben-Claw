"""Replace one crate-wide `allow` with a handful of narrow, reasoned ones.

The remaining dead-code warnings are almost all wire-format types: structs
deserialised from a chat platform's webhook payload, where a field exists to
document what the payload contains even when this code never reads it. Deleting
those fields would make the type a worse description of the wire, so `allow` is
the right answer — at the file, with a reason, not at the crate root where it
also hid five unreachable modules.

Anything outside that pattern is left warning on purpose. A short list of real
warnings is worth more than a clean build that means nothing.
"""

import pathlib
import sys

REASON = (
    "// Wire-format types: fields mirror the platform's webhook payload and exist to\n"
    "// document what arrives, even where this code does not read them. Deleting them\n"
    "// would make the struct a worse description of the wire than the vendor's own docs.\n"
    "// Scoped to this file deliberately — the crate root carries no blanket allow.\n"
    "#![allow(dead_code)]\n"
)

root = pathlib.Path(sys.argv[1])
targets = [
    "src/channels/feishu.rs",
    "src/channels/telegram.rs",
    "src/channels/imessage.rs",
    "src/channels/slack.rs",
    "src/channels/matrix.rs",
]

for rel in targets:
    p = root / rel
    if not p.exists():
        print(f"  {rel}: missing")
        continue
    s = p.read_text(encoding="utf-8")
    if "#![allow(dead_code)]" in s:
        print(f"  {rel}: already scoped")
        continue
    # Inner attributes must precede everything except inner doc comments, which
    # these files open with.
    lines = s.splitlines(keepends=True)
    i = 0
    while i < len(lines) and (lines[i].startswith("//!") or not lines[i].strip()):
        i += 1
    lines.insert(i, REASON)
    p.write_text("".join(lines), encoding="utf-8")
    print(f"  {rel}: scoped allow inserted at line {i + 1}")
print("done")
