"""Gate 3: unwire the cut modules, and stop the crate hiding dead code.

Two separable things, done together because the second is what makes the first
stay true. Deleting five unreachable modules is a one-time tidy; removing the
crate-level `allow(dead_code, ...)` is what stops the next five from becoming
invisible the same way.
"""

import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
CUT = ["a2a", "dashboard", "rag", "satcom", "hooks"]


def edit(rel, fn):
    p = root / rel
    s = p.read_text(encoding="utf-8")
    out = fn(s)
    if out != s:
        p.write_text(out, encoding="utf-8")
        print(f"  {rel}: updated")
    else:
        print(f"  {rel}: no change")


def lib_rs(s):
    # The blanket allow is the finding. A library does legitimately expose more
    # surface than its binary uses — but that is an argument for `pub`, not for
    # silencing the lint that would have named 2,700 lines of unreachable code.
    s = s.replace(
        "#![allow(dead_code, unused_imports, unused_variables)]\n",
        "",
    )
    s = s.replace(
        "// Public library API",
        "// NOTE: this crate deliberately does NOT carry a blanket\n"
        "// `#![allow(dead_code, unused_imports, unused_variables)]`. It used to, and that\n"
        "// is how five unreachable modules and a documented-but-never-called\n"
        "// PersonalityStore sat here without a single warning. Suppress narrowly, at the\n"
        "// item, with a reason — never at the crate root.\n"
        "//\n"
        "// Public library API",
    )
    for m in CUT:
        s = re.sub(rf"^pub mod {m};\n", "", s, flags=re.M)
    return s


def memory_mod(s):
    s = re.sub(r"^pub mod personality;\n", "", s, flags=re.M)
    s = re.sub(r"^pub use personality::PersonalityStore;\n", "", s, flags=re.M)
    s = re.sub(r"^.*\[`personality`\].*\n", "", s, flags=re.M)
    return s


edit("src/lib.rs", lib_rs)
edit("src/memory/mod.rs", memory_mod)
print("done")
