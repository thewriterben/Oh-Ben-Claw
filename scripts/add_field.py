"""One-off: add `fire_on_change: false` to every ReflexRule literal.

The field is #[serde(default)] on the wire, but a struct literal has to name every
field, so the default only helps deserialization. Explicit `false` at each site is
also the honest outcome: opting a rule in is a decision, not something a mechanical
edit should make on anyone's behalf.
"""

import pathlib
import re
import sys

total = 0
for p in sys.argv[1:]:
    f = pathlib.Path(p)
    s = f.read_text(encoding="utf-8")
    n = 0

    def sub(m):
        global n
        n += 1
        return m.group(0) + "\n" + m.group(1) + "fire_on_change: false,"

    s = re.sub(r"(?m)^([ \t]*)max_rate_hz: (?:None|Some\([^)]*\)),$", sub, s)
    f.write_text(s, encoding="utf-8")
    print(f"{p}: {n} rule literals updated")
    total += n
print(f"total: {total}")
