#!/usr/bin/env python3
"""Take a picture off a camera node and save it as a .jpg you can open.

    python scripts/grab_picture.py --port COM11 [--quality 8] [--count 1]
    python scripts/grab_picture.py --selftest

The node captures greyscale for the detector and encodes a picture in software
(`fmt2jpg_cb`, camera.rs), so what comes back is a monochrome JPEG. A probe that
counts `frame len=76800` lines proves the sensor delivered pixels; it does not
prove the encoder turned them into an image. This does: it checks the bytes
start with FFD8 and end with FFD9, reads the width and height out of the
JPEG's own frame header, and writes the file for a person to look at.

The port is asked for `capabilities` first and the run stops, before any
camera command, if the node is obc-esp32-s3-001 (the live mesh node). Find the
port with scripts/which_esp32.ps1.

Files land in results/pictures/<node>-<stamp>-q<quality>[-n].jpg.
"""

from __future__ import annotations

import argparse
import base64
import datetime
import json
import os
import re
import sys
import time

LIVE_NODE = "obc-esp32-s3-001"
NODE = re.compile(r"obc-esp32-s3-[0-9a-z]{3,6}")


def jpeg_facts(data: bytes) -> dict:
    """What the bytes say about themselves. `ok` only if SOI, EOI and a frame header are all there."""
    facts = {"bytes": len(data), "soi": data[:2] == b"\xff\xd8", "eoi": data[-2:] == b"\xff\xd9"}
    i = 2
    while facts["soi"] and i + 4 <= len(data):
        if data[i] != 0xFF:
            break
        marker = data[i + 1]
        if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
            i += 2
            continue
        seglen = int.from_bytes(data[i + 2:i + 4], "big")
        # SOF0..SOF15, excluding DHT (C4), JPG (C8) and DAC (CC).
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC) and i + 9 <= len(data):
            facts["height"] = int.from_bytes(data[i + 5:i + 7], "big")
            facts["width"] = int.from_bytes(data[i + 7:i + 9], "big")
            facts["components"] = data[i + 9] if i + 9 < len(data) else None
            break
        if marker == 0xDA:  # start of scan: no frame header before it
            break
        i += 2 + seglen
    facts["ok"] = facts["soi"] and facts["eoi"] and "width" in facts
    return facts


class Link:
    def __init__(self, port: str):
        import serial  # only needed for a live run

        self.ser = serial.Serial(port, 115200, timeout=0.3)
        time.sleep(3.5)  # opening the port may reset the board
        self.ser.reset_input_buffer()

    def request(self, ident: str, cmd: str, args: dict, timeout: float = 25.0) -> dict | None:
        self.ser.write((json.dumps({"id": ident, "cmd": cmd, "args": args}) + "\n").encode())
        self.ser.flush()
        end, buf = time.time() + timeout, b""
        while time.time() < end:
            chunk = self.ser.read(16384)
            if not chunk:
                continue
            buf += chunk
            if f'"{ident}"'.encode() in buf and buf.rstrip().endswith(b"}"):
                break
        for line in buf.decode("utf-8", "replace").splitlines():
            if f'"{ident}"' not in line or "{" not in line:
                continue
            try:
                obj = json.loads(line[line.index("{"):])
            except ValueError:
                continue
            if isinstance(obj, dict) and str(obj.get("id")) == ident:
                return obj
        return None


def selftest() -> int:
    fails = []
    # A minimal baseline JPEG: SOI, APP0, SOF0 (8-bit, 240x320, 1 component), SOS, EOI.
    sof = b"\xff\xc0\x00\x0b\x08\x00\xf0\x01\x40\x01\x01\x11\x00"
    app0 = b"\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00"
    good = b"\xff\xd8" + app0 + sof + b"\xff\xda\x00\x08\x01\x01\x00\x00\x3f\x00" + b"\x12\x34" + b"\xff\xd9"
    f = jpeg_facts(good)
    if not (f["ok"] and f["width"] == 320 and f["height"] == 240 and f["components"] == 1):
        fails.append(f"good jpeg: {f}")
    if jpeg_facts(good[:-2])["ok"]:
        fails.append("truncated (no EOI) passed")
    if jpeg_facts(b"\x00" * 76800)["ok"]:
        fails.append("raw Y8 passed as a JPEG")
    if jpeg_facts(b"\xff\xd8" + app0 + b"\xff\xda\x00\x02" + b"\xff\xd9")["ok"]:
        fails.append("no frame header passed")
    here = os.path.dirname(os.path.abspath(__file__))
    fixture = os.path.join(here, "..", "tests", "fixtures", "vision-floor-2026-09-17", "first.jpg")
    if os.path.isfile(fixture):
        with open(fixture, "rb") as fh:
            f = jpeg_facts(fh.read())
        if not (f["ok"] and f["width"] == 320 and f["height"] == 240):
            fails.append(f"on-node fixture first.jpg: {f}")
    if NODE.search(json.dumps({"result": '{"node_id":"obc-esp32-s3-005"}'})).group(0) != "obc-esp32-s3-005":
        fails.append("node id not found in a capabilities reply")
    if fails:
        print("SELFTEST FAILED\n  " + "\n  ".join(fails))
        return 1
    print("selftest ok: SOI/EOI/frame-header checks, truncation and raw-Y8 refused, "
          "the 2026-09-17 on-node picture reads as 320x240")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--port", help="the camera board's port, as named by scripts/which_esp32.ps1")
    ap.add_argument("--quality", type=int, default=8, help="1..10, the node's own scale (higher is better)")
    ap.add_argument("--count", type=int, default=1)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.port:
        ap.error("--port is required: run scripts/which_esp32.ps1 and pass the camera board's port")

    link = Link(args.port)
    who_reply = link.request("who", "capabilities", {}, timeout=5)
    m = NODE.search(json.dumps(who_reply)) if who_reply else None
    node = m.group(0) if m else None
    if node == LIVE_NODE:
        sys.exit(f"{args.port} is {LIVE_NODE}, the live mesh node. No camera command sent.")
    print(f"{args.port}: {node or 'node unidentified (no capabilities reply)'}")

    out_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "results", "pictures")
    os.makedirs(out_dir, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    short = (node or "unknown").replace("obc-esp32-s3-", "")
    good = 0
    for n in range(args.count):
        reply = link.request(f"pic{n}", "camera_capture", {"quality": args.quality})
        if not reply or not reply.get("ok"):
            print(f"  {n + 1}/{args.count}: no picture -- {reply.get('error') if reply else 'no reply within 25 s'}")
            continue
        try:
            data = base64.b64decode(reply["result"], validate=True)
        except (ValueError, TypeError) as e:
            print(f"  {n + 1}/{args.count}: result is not base64 ({e}); starts {str(reply.get('result'))[:40]!r}")
            continue
        f = jpeg_facts(data)
        suffix = f"-{n + 1}" if args.count > 1 else ""
        path = os.path.join(out_dir, f"{short}-{stamp}-q{args.quality}{suffix}.jpg")
        with open(path, "wb") as fh:
            fh.write(data)
        verdict = "JPEG" if f["ok"] else "NOT A COMPLETE JPEG"
        dims = f"{f.get('width', '?')}x{f.get('height', '?')}, {f.get('components', '?')} component(s)"
        print(f"  {n + 1}/{args.count}: {verdict}  {f['bytes']} B  {dims}\n      {os.path.normpath(path)}")
        good += f["ok"]
    print(f"\n  {good}/{args.count} complete JPEGs. Open them: a picture is the check, not the byte count.")
    return 0 if good == args.count else 1


if __name__ == "__main__":
    sys.exit(main())
