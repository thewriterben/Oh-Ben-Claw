#!/usr/bin/env python3
"""The camera crate builds the same firmware as the node crate, plus the camera.

    python scripts/check_camera_crate_drift.py
    python scripts/check_camera_crate_drift.py --selftest

# Why

`firmware/obc-esp32-s3-camera` has no sources. It builds
`firmware/obc-esp32-s3/src/main.rs` with the esp32-camera IDF component added,
which is the only way to add that component without it reaching the live
node's build (see that crate's Cargo.toml, and check_camera_component_gate.py).

That makes it a second manifest for one program, and two manifests drift. A
dependency bumped in one and not the other gives the camera build a different
esp-idf-svc from the node build -- the binaries on 002 and 005 would stop being
"001's firmware plus a camera", and nothing would say so. This compares what
must be equal and checks what must differ.

# What is compared

  equal     [dependencies], [build-dependencies], every feature except
            `default`, the bin name, `.cargo/config.toml`,
            `rust-toolchain.toml`
  camera    default = ["camera"]; the bin and build script point into
            ../obc-esp32-s3; the component block is live, and its
            bindings_header resolves to a file that exists
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NODE = ROOT / "firmware" / "obc-esp32-s3"
CAM = ROOT / "firmware" / "obc-esp32-s3-camera"
SHARED_FILES = [".cargo/config.toml", "rust-toolchain.toml"]


def compare(node: dict, cam: dict, cam_dir: Path, files: dict[str, tuple[str, str]]) -> list[str]:
    """Every way the camera manifest has left the node manifest. Empty is good."""
    errs = []
    for table in ("dependencies", "build-dependencies"):
        if node.get(table) != cam.get(table):
            errs.append(f"[{table}] differs:\n    node   {node.get(table)}\n    camera {cam.get(table)}")

    nf = dict(node.get("features", {}))
    cf = dict(cam.get("features", {}))
    if cf.pop("default", None) != ["camera"]:
        errs.append('camera crate must have default = ["camera"]')
    nf.pop("default", None)
    if nf != cf:
        only_node = sorted(set(nf) - set(cf))
        only_cam = sorted(set(cf) - set(nf))
        changed = sorted(k for k in set(nf) & set(cf) if nf[k] != cf[k])
        errs.append(f"[features] differ: only in node {only_node}, only in camera {only_cam}, "
                    f"different {changed}")

    nb = [b.get("name") for b in node.get("bin", [])]
    cb = cam.get("bin", [])
    if [b.get("name") for b in cb] != nb:
        errs.append(f"bin names differ: node {nb}, camera {[b.get('name') for b in cb]}")
    if any(b.get("path") != "../obc-esp32-s3/src/main.rs" for b in cb):
        errs.append("camera bin must be ../obc-esp32-s3/src/main.rs (no sources of its own)")
    if cam.get("package", {}).get("build") != "../obc-esp32-s3/build.rs":
        errs.append("camera crate must use build = \"../obc-esp32-s3/build.rs\"")

    comps = cam.get("package", {}).get("metadata", {}).get("esp-idf-sys", {}).get("extra_components", [])
    if not any(c.get("remote_component", {}).get("name") == "espressif/esp32-camera" for c in comps):
        errs.append("camera crate has no live espressif/esp32-camera extra_components block")
    for c in comps:
        h = c.get("bindings_header")
        if h and not (cam_dir / h).resolve().is_file():
            errs.append(f"bindings_header {h} does not resolve from {cam_dir.name}/")

    for name, (a, b) in files.items():
        if a != b:
            errs.append(f"{name} differs between the two crates")
    return errs


def load() -> tuple[dict, dict, dict[str, tuple[str, str]]]:
    node = tomllib.loads((NODE / "Cargo.toml").read_text(encoding="utf-8"))
    cam = tomllib.loads((CAM / "Cargo.toml").read_text(encoding="utf-8"))
    files = {}
    for f in SHARED_FILES:
        a, b = NODE / f, CAM / f
        files[f] = (a.read_text(encoding="utf-8") if a.is_file() else "<missing>",
                    b.read_text(encoding="utf-8") if b.is_file() else "<missing>")
    return node, cam, files


def selftest() -> int:
    node, cam, files = load()
    fails = []
    if compare(node, cam, CAM, files):
        fails.append("the tree as committed should pass")

    def mutated(fn) -> list[str]:
        import copy
        n, c, f = copy.deepcopy(node), copy.deepcopy(cam), dict(files)
        fn(n, c, f)
        return compare(n, c, CAM, f)

    cases = {
        "a dependency bumped in the node only":
            lambda n, c, f: n["dependencies"].__setitem__("esp-idf-hal", "0.47"),
        "a feature added to the node only":
            lambda n, c, f: n["features"].__setitem__("board-new", []),
        "camera no longer default":
            lambda n, c, f: c["features"].__setitem__("default", []),
        "the component block dropped":
            lambda n, c, f: c["package"]["metadata"]["esp-idf-sys"].pop("extra_components"),
        "a bindings header that is not there":
            lambda n, c, f: c["package"]["metadata"]["esp-idf-sys"]["extra_components"][0]
            .__setitem__("bindings_header", "src/camera_bindings.h"),
        "a bin of its own":
            lambda n, c, f: c["bin"][0].__setitem__("path", "src/main.rs"),
        "the toolchain pin diverged":
            lambda n, c, f: f.__setitem__("rust-toolchain.toml", ("a", "b")),
    }
    for name, fn in cases.items():
        if not mutated(fn):
            fails.append(f"not caught: {name}")
    if fails:
        print("SELFTEST FAILED\n  " + "\n  ".join(fails))
        return 1
    print(f"selftest ok: the committed tree passes, and {len(cases)} kinds of drift are each caught")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    for d in (NODE, CAM):
        if not (d / "Cargo.toml").is_file():
            print(f"not found: {d.relative_to(ROOT)}/Cargo.toml")
            return 2
    node, cam, files = load()
    errs = compare(node, cam, CAM, files)
    if errs:
        print("the camera crate has drifted from the node crate:\n  " + "\n  ".join(errs))
        return 1
    print("ok: obc-esp32-s3-camera builds obc-esp32-s3's sources with the same deps, "
          "features, toolchain and cargo config, plus the camera component")
    return 0


if __name__ == "__main__":
    sys.exit(main())
