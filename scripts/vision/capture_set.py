"""Capture a labelled run of frames from the camera node, for detector fixtures.

    python capture_set.py <label> <count> [interval_s]

Writes obc-bench/dataset/<label>/NNN.jpg and appends to dataset/manifest.jsonl.

Why a manifest: a detector tuned against frames with no record of what was
happening is tuned against nothing. Each row carries the label, the wall clock,
the node's own ts_ms, and the JPEG length -- so a later analysis can line a score
up against what the room was actually doing, rather than against my memory of it.

COM10 only, by construction: COM6 is the live mesh node.
"""

import base64
import json
import os
import sys
import time

import serial

PORT = "COM10"
ROOT = r"C:\Users\Benji\obc-bench\dataset"

label = sys.argv[1] if len(sys.argv) > 1 else "unlabelled"
count = int(sys.argv[2]) if len(sys.argv) > 2 else 30
interval = float(sys.argv[3]) if len(sys.argv) > 3 else 0.5

outdir = os.path.join(ROOT, label)
os.makedirs(outdir, exist_ok=True)
manifest = os.path.join(ROOT, "manifest.jsonl")

print(f"label={label!r} count={count} interval={interval}s -> {outdir}", flush=True)

kept = 0
failed = 0
with serial.Serial(PORT, 115200, timeout=0.3) as ser:
    time.sleep(3.5)          # let the open-reset settle
    ser.reset_input_buffer()
    t_start = time.time()
    for i in range(count):
        req = {"id": f"d{i}", "cmd": "camera_capture", "args": {"quality": 10}}
        ser.write((json.dumps(req) + "\n").encode())
        ser.flush()
        end = time.time() + 15
        buf = b""
        got = False
        while time.time() < end:
            c = ser.read(16384)
            if not c:
                continue
            buf += c
            if f'"d{i}"'.encode() in buf and buf.rstrip().endswith(b"}"):
                break
            if b"reply_dropped" in buf:
                break
        for line in buf.decode("utf-8", "replace").splitlines():
            t = line.strip()
            if f'"d{i}"' not in t:
                continue
            try:
                o = json.loads(t)
            except Exception:
                continue
            if not o.get("ok"):
                print(f"  {i:03d} FAIL {o.get('error')}")
                failed += 1
                got = True
                break
            data = base64.b64decode(o["result"])
            if data[:2] != b"\xff\xd8":
                print(f"  {i:03d} not a JPEG ({len(data)} B)")
                failed += 1
                got = True
                break
            name = f"{i:03d}.jpg"
            with open(os.path.join(outdir, name), "wb") as fh:
                fh.write(data)
            with open(manifest, "a", encoding="utf-8") as mf:
                mf.write(json.dumps({
                    "label": label,
                    "index": i,
                    "file": os.path.join(label, name).replace("\\", "/"),
                    "bytes": len(data),
                    "wall": time.time(),
                    "elapsed_s": round(time.time() - t_start, 3),
                }) + "\n")
            kept += 1
            got = True
            break
        if not got:
            print(f"  {i:03d} NO REPLY")
            failed += 1
        time.sleep(interval)

print(f"\nkept {kept}, failed {failed}, into {outdir}")
sys.exit(0 if kept else 1)
