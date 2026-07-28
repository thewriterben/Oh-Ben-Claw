"""Adapt the api_key call sites to SecretString.

The three cloud providers share one shape:

    config.api_key.clone().or_else(|| env::var("X_API_KEY").ok())

`.clone()` now yields Option<SecretString>, so the env fallback no longer unifies.
`.as_ref().map(|k| k.expose().to_string())` restores it, and `expose` is the point:
reaching the raw value reads as a deliberate act at every site.
"""

import pathlib
import re
import sys

total = 0
for p in sys.argv[1:]:
    f = pathlib.Path(p)
    s = f.read_text(encoding="utf-8")
    before = s
    s = re.sub(
        r"\.api_key\s*\n(\s*)\.clone\(\)",
        lambda m: f".api_key\n{m.group(1)}.as_ref()\n{m.group(1)}.map(|k| k.expose().to_string())",
        s,
    )
    s = s.replace(
        "let api_key = config.api_key.clone();",
        "let api_key = config.api_key.as_ref().map(|k| k.expose().to_string());",
    )
    if s != before:
        f.write_text(s, encoding="utf-8")
        n = len(re.findall(r"expose\(\)", s))
        print(f"{p}: patched ({n} expose call(s))")
        total += 1
    else:
        print(f"{p}: no change")
print(f"{total} file(s) changed")
