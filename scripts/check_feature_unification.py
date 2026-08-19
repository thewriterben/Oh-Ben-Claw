#!/usr/bin/env python3
"""A dependency whose defaults the root turns off *for a stated reason* must
have them turned off in every manifest that names it.

Cargo unifies features across the workspace graph. `default-features = false`
in one manifest is not a decision that manifest can make alone: if any other
crate asks for the defaults, everything gets them.

Why "for a stated reason" rather than "anywhere". The root turns defaults off
on a dozen dependencies, mostly to keep `std` out of wasm builds, and twenty-two
crate manifests do not repeat that -- harmlessly, because those crates are never
built for wasm. A gate flagging all of them reports forty-four problems to find
one, and a gate that cries wolf is a gate someone turns off. So the signal is
the repository's own convention: when the reason is load-bearing, the root
manifest says so in a comment directly above the line. That comment is the
declaration that the flag matters.

It went wrong once, on 2026-08-14. crates/obc-spine declared a bare
`rumqttc = "0.24"` against a root that had disabled its defaults under a
seven-line comment naming four RUSTSEC advisories in rustls-webpki 0.102.8.
Feature unification put the whole vulnerable certificate stack back. `cargo
check`, `clippy`, `fmt` and 1556 tests stayed green -- nothing about the build
was wrong. Only `cargo audit` could see it, and only after the push.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(".")
DECL = re.compile(r"^([A-Za-z0-9_-]+)\s*=\s*(.+)$")
NO_DEFAULTS = re.compile(r"default-features\s*=\s*false")


def declarations(path):
    """dep name -> (defaults_disabled, reason_comment_lines)."""
    out, section, comment = {}, None, []
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line.startswith("["):
            section, comment = line.strip("[]"), []
            continue
        if line.startswith("#"):
            # A bare section banner is not a reason; prose is.
            comment.append(line.lstrip("# ").rstrip())
            continue
        m = DECL.match(raw)
        if not m or section is None or "dependencies" not in section:
            if not line:
                comment = []
            continue
        out[m.group(1)] = (bool(NO_DEFAULTS.search(m.group(2))), comment)
        comment = []
    return out


root = declarations(ROOT / "Cargo.toml")
# Load-bearing: defaults off, and the root says why in more than a banner.
guarded = {
    name: " ".join(reason)
    for name, (off, reason) in root.items()
    if off and sum(len(r) for r in reason) > 80
}

bad = []
for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
    for name, (off, _) in declarations(manifest).items():
        if name in guarded and not off:
            bad.append((manifest.as_posix(), name))

for path, name in bad:
    print(f"  {path}: `{name}` takes default features.")
    print(f"      root: {guarded[name][:150]}...")

if bad:
    print(f"\n{len(bad)} manifest(s) re-enable defaults the root turns off on "
          "purpose. Feature unification means one is enough to undo all of them.")
    sys.exit(1)
print(f"ok: {len(guarded)} dependency(ies) with a stated reason for "
      "`default-features = false`, and every manifest repeats it")
