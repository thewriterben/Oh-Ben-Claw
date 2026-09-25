#!/usr/bin/env python3
"""Does THIS board produce a frame? One camera board, identified and tried N times.

    python scripts/probe_sense_capture.py --port COM11 [--count 6] [--label "..."]
    python scripts/probe_sense_capture.py --selftest

Written for the XIAO ESP32S3 Sense capture fault (camera.rs, "OPEN: init
succeeds, capture never does"). On obc-esp32-s3-002, `esp_camera_init` succeeds
and the OV2640 answers over SCCB, but every `esp_camera_fb_get` returns null
after exactly the driver's 4000 ms timeout. The Lilygo, running the same driver,
captures, so the driver itself is not the suspect. What is left is this one
board: its expansion board, its connector, its sensor.

A second, complete XIAO Sense is the discriminating experiment. The rule that
makes it discriminating: **both boards run the same binary, back to back.**
002's failure was recorded on the 2026-09-16 JPEG build, and camera-bringup has
since moved to greyscale capture. A "works on the new board" against a failure
on an older build compares two builds, not two boards. So flash one binary, run
this against the new Sense, re-flash 002 with that same binary, and run this
against 002.

What it records, all from the node's own console plus the command replies:

  identity   `Node ID: … (MAC …)` from the boot banner
  sensor     `Detected … camera` and `Camera PID=…`
  psram      the memory-test line, so a wrong overlay is visible
  per try    frame   `capture: frame len=N B, WxH, format=F`
             null    `esp_camera_fb_get returned null` / `Failed to get the frame on time`
             stub    the build has no `camera` feature, so this measured nothing
             silent  none of the above within the timeout

Opening the port resets the board, which is how the boot banner is captured.
That is also why there is no default port: **run scripts/which_esp32.ps1 first**
and pass the port it names. If the banner turns out to be the live mesh node
anyway, the run stops before sending anything.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import sys
import time

ANSI = re.compile(r"\x1b\[[0-9;]*m")
LIVE_NODE = "obc-esp32-s3-001"

NODE = re.compile(r"Node ID: (\S+)\s+\(MAC ([0-9A-Fa-f:]{17})\)")
SENSOR = re.compile(r"Detected (\S+) camera")
PID = re.compile(r"Camera PID=(0x[0-9A-Fa-f]+)")
PSRAM_OK = re.compile(r"esp_psram: SPI SRAM memory test OK")
PSRAM_POOL = re.compile(r"Adding pool of (\d+)K of PSRAM")
CAM_INIT_FAIL = re.compile(r"camera init failed \((.*)\)")
FRAME = re.compile(r"capture: frame len=(\d+) B, (\d+)x(\d+), format=(\d+)")
NODE_IN_REPLY = re.compile(r"obc-esp32-s3-[0-9a-z]{3,6}")
NULL = re.compile(r"esp_camera_fb_get returned null|Failed to get the frame on time")


def classify(lines: list[str], reply: dict | None) -> dict:
    """One attempt's console lines and its reply (if any) -> its outcome."""
    frame = next((m for l in lines if (m := FRAME.search(l))), None)
    if frame:
        return {"outcome": "frame", "bytes": int(frame.group(1)),
                "size": f"{frame.group(2)}x{frame.group(3)}", "format": int(frame.group(4))}
    result = reply.get("result") if reply else None
    if isinstance(result, str) and result.startswith("STUB:"):
        return {"outcome": "stub"}
    if any(NULL.search(l) for l in lines) or (reply and "null" in str(reply.get("error", ""))):
        return {"outcome": "null"}
    return {"outcome": "silent"}


def banner_facts(lines: list[str]) -> dict:
    facts: dict = {}
    for l in lines:
        if m := NODE.search(l):
            facts["node_id"], facts["mac"] = m.group(1), m.group(2).upper()
        if m := SENSOR.search(l):
            facts["sensor"] = m.group(1)
        if m := PID.search(l):
            facts["pid"] = m.group(1)
        if PSRAM_OK.search(l):
            facts["psram_test"] = "ok"
        if m := PSRAM_POOL.search(l):
            facts["psram_kb"] = int(m.group(1))
        if m := CAM_INIT_FAIL.search(l):
            facts["camera_init_error"] = m.group(1)
    return facts


class Console:
    def __init__(self, port: str):
        import serial  # only needed for a live run

        self.ser = serial.Serial(port, 115200, timeout=0.05)
        self.buf = b""

    def lines(self, seconds: float) -> list[str]:
        out, end = [], time.time() + seconds
        while time.time() < end:
            chunk = self.ser.read(4096)
            if not chunk:
                continue
            self.buf += chunk
            *whole, self.buf = self.buf.split(b"\n")
            out += [ANSI.sub("", w.decode("utf-8", errors="replace")).rstrip() for w in whole if w.strip()]
        return out

    def send(self, obj: dict) -> None:
        self.ser.write((json.dumps(obj) + "\n").encode())


def reply_for(lines: list[str], req_id: str) -> dict | None:
    for l in lines:
        s = l[l.find("{"):] if "{" in l else ""
        try:
            obj = json.loads(s)
        except ValueError:
            continue
        if isinstance(obj, dict) and str(obj.get("id")) == req_id:
            return obj
    return None


def node_from_reply(reply: dict | None) -> str | None:
    """The node id inside a `capabilities` reply, whatever shape its result takes."""
    m = NODE_IN_REPLY.search(json.dumps(reply)) if reply else None
    return m.group(0) if m else None


def run(port: str, count: int, boot_s: float, try_s: float) -> dict:
    con = Console(port)
    boot = con.lines(boot_s)
    facts = banner_facts(boot)
    if "node_id" not in facts:
        # On 2026-09-25 a port opened without resetting the board, so no banner
        # came. `capabilities` is read-only and names the node, which is enough
        # to refuse the live one before any camera command is sent.
        con.send({"id": "who", "cmd": "capabilities"})
        who = node_from_reply(reply_for(con.lines(3.0), "who"))
        if who:
            facts["node_id"] = who
            facts["identity_from"] = "capabilities"
        else:
            print("  ! no `Node ID:` banner and no `capabilities` reply -- "
                  "continuing, but identity is unrecorded.")
    if facts.get("node_id") == LIVE_NODE:
        sys.exit(f"{port} is {LIVE_NODE}, the live mesh node. No camera command sent. "
                 "Run scripts/which_esp32.ps1 and pass the camera board's port.")
    tries = []
    try:
        for i in range(count):
            req_id = f"cap{i}"
            t0 = time.time()
            con.send({"id": req_id, "cmd": "camera_capture", "args": {"quality": 12}})
            seen = con.lines(try_s)
            reply = reply_for(seen, req_id)
            row = classify(seen, reply)
            row["seconds"] = round(time.time() - t0, 2)
            row["reply_ok"] = reply.get("ok") if reply else None
            tries.append(row)
            print(f"  try {i + 1}/{count}: {row['outcome']}"
                  + (f" {row['bytes']} B {row['size']} format={row['format']}" if row["outcome"] == "frame" else ""),
                  flush=True)
    except KeyboardInterrupt:
        # A run stopped by hand still measured something; keep it.
        print(f"  interrupted after {len(tries)} of {count} tries -- keeping what was measured")
    return {"port": port, "boot": facts, "tries": tries,
            "frames": sum(1 for t in tries if t["outcome"] == "frame"),
            "count": len(tries), "requested": count}


def selftest() -> int:
    fails = []

    def check(name, got, want):
        if got != want:
            fails.append(f"{name}: got {got!r}, want {want!r}")

    boot = [
        "\x1b[0;32mI (914) esp_psram: SPI SRAM memory test OK\x1b[0m",
        "I (998) esp_psram: Adding pool of 8192K of PSRAM memory to heap allocator",
        "I (1200) camera: Detected OV2640 camera",
        "I (1201) camera: Camera PID=0x26 VER=0x42 MIDL=0x7f MIDH=0xa2",
        "I (1500) obc_esp32_s3: Node ID: obc-esp32-s3-002  (MAC 64:e8:33:7e:7e:04)",
    ]
    check("banner", banner_facts(boot),
          {"psram_test": "ok", "psram_kb": 8192, "sensor": "OV2640", "pid": "0x26",
           "node_id": "obc-esp32-s3-002", "mac": "64:E8:33:7E:7E:04"})
    check("frame", classify(["I (9) obc_esp32_s3::camera: capture: frame len=76800 B, 320x240, format=2, base64 will be ~4000 B"],
                            {"id": "cap0", "ok": True, "result": "/9j/"}),
          {"outcome": "frame", "bytes": 76800, "size": "320x240", "format": 2})
    check("null via cam_hal", classify(["W (4000) cam_hal: Failed to get the frame on time!"], None),
          {"outcome": "null"})
    check("null via reply", classify([], {"id": "cap0", "ok": False,
                                          "error": "esp_camera_fb_get returned null (no frame)"}),
          {"outcome": "null"})
    check("stub", classify([], {"id": "cap0", "ok": True,
                                "result": "STUB:camera_capture:quality=12:format=jpeg:base64_jpeg_data_here"}),
          {"outcome": "stub"})
    check("silent", classify(["I (1) something else"], None), {"outcome": "silent"})
    check("reply id match", reply_for(['noise', '{"id":"cap1","ok":true}', '{"id":"cap2","ok":false}'], "cap2"),
          {"id": "cap2", "ok": False})
    check("node from capabilities reply",
          node_from_reply({"id": "who", "ok": True, "result": '{"node_id":"obc-esp32-s3-002","board":"xiao"}'}),
          "obc-esp32-s3-002")
    check("no node in reply", node_from_reply({"id": "who", "ok": False, "error": "x"}), None)
    check("live node named", banner_facts(["Node ID: obc-esp32-s3-001  (MAC 64:E8:33:7E:BB:98)"])["node_id"], LIVE_NODE)
    if fails:
        print("SELFTEST FAILED\n  " + "\n  ".join(fails))
        return 1
    print("selftest ok: banner facts, frame/null/stub/silent outcomes, reply matching, "
          "identity from capabilities, live-node name")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--port", help="the camera board's port, as named by scripts/which_esp32.ps1")
    ap.add_argument("--count", type=int, default=6)
    ap.add_argument("--boot-seconds", type=float, default=6.0, help="how long to read the boot banner")
    ap.add_argument("--try-seconds", type=float, default=8.0, help="per capture; the driver's own timeout is 4 s")
    ap.add_argument("--label", default="", help="free text, e.g. 'second Sense, build <hash>'")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.port:
        ap.error("--port is required: run scripts/which_esp32.ps1 and pass the camera board's port")

    print(f"{args.port}: reading boot banner, then {args.count} captures")
    r = run(args.port, args.count, args.boot_seconds, args.try_seconds)
    r["label"] = args.label
    b = r["boot"]
    print(f"\n  board   {b.get('node_id', '?')}  MAC {b.get('mac', '?')}")
    print(f"  sensor  {b.get('sensor', '?')} PID {b.get('pid', '?')}   "
          f"psram {b.get('psram_test', '?')} {b.get('psram_kb', '?')}K"
          + (f"   CAMERA INIT FAILED: {b['camera_init_error']}" if "camera_init_error" in b else ""))
    print(f"  frames  {r['frames']}/{r['count']}")

    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "results")
    os.makedirs(root, exist_ok=True)
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    who = b.get("node_id", "unknown").replace("obc-esp32-s3-", "")
    out = os.path.join(root, f"probe_sense_capture-{who}-{stamp}.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(r, f, indent=2)
    print(f"  wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
