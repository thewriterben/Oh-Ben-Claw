#!/usr/bin/env python3
"""SPINE-REPLAY.md section 6, steps 4 and 5, on a Heltec station (the bridge,
which the brain does not hold).

  step 4  A crash loop is survivable. Reset the station N times at a fixed
          interval (RTS with DTR low: scripts/probe_reset_station.py's trick)
          and read the boot line each time:
              frame counter resumed at C (ceiling persisted); nvs entries used=U free=F total=T
          The design says a boot costs one NVS write and RESERVE (32) counter
          numbers, plus one more write per 32 frames sent between boots. So
          C[k+1] - C[k] == 32 + 32 * (frames sent in interval k // 32), never
          less, and never a repeat; and `free` falls by the number of writes
          (until a page is reclaimed, which shows as a jump back up).

  step 5  NVS failure stops transmission. Send {"bench":"nvs_fault"} on the
          console (needs a `bench-nvs-fault` build), then confirm the station
          reports TRANSMIT REFUSED within 32 frames and goes silent on the air:
          the brain's world.db shows mesh.gw-40 stop refreshing, health goes
          offline after stale_ms, and escalation follows escalate_after_ms.
          Then reset the station and confirm it comes back and the escalation
          clears.

    python scripts/bench_seq_wear.py --port COM7 --station gw-40 step4 --resets 10 --interval 4
    python scripts/bench_seq_wear.py --port COM7 --station gw-40 step5 --watch 300

Opens the station's port with DTR/RTS low (no reset on open). Records under
results/.
"""
import argparse, datetime, json, os, re, shutil, sqlite3, sys, tempfile, time

import serial

RESERVE = 32
BOOT_RE = re.compile(r"frame counter resumed at (\d+) \(ceiling persisted\)(?:; nvs entries used=(\d+) free=(\d+) total=(\d+))?")
DATA = os.path.join(os.environ.get("APPDATA", ""), r"thewriterben\oh-ben-claw\data")


def stamp():
    return datetime.datetime.now().strftime("%H:%M:%S")


def open_quiet(port):
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.2
    s.dtr = False
    s.rts = False
    s.open()
    s.dtr = False
    s.rts = False
    return s


def reset(s):
    """Reset a Heltec V3 through the CP2102: RTS asserted with DTR low pulls EN."""
    s.dtr = False
    s.rts = True
    time.sleep(0.1)
    s.rts = False


def read_lines(s, seconds):
    out, deadline, buf = [], time.time() + seconds, b""
    while time.time() < deadline:
        chunk = s.read(4096)
        if chunk:
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                out.append(line.decode("utf-8", "replace").rstrip("\r"))
    return out


def strip_ansi(s):
    return re.sub(r"\x1b\[[0-9;]*m", "", s)


def wait_boot_line(s, timeout):
    """Return (count, used, free, total, frames_seen_before_boot_line_end) or None."""
    deadline = time.time() + timeout
    buf = b""
    while time.time() < deadline:
        chunk = s.read(4096)
        if not chunk:
            continue
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            m = BOOT_RE.search(strip_ansi(line.decode("utf-8", "replace")))
            if m:
                c = int(m.group(1))
                u, f, t = (int(m.group(i)) if m.group(i) else None for i in (2, 3, 4))
                return c, u, f, t
    return None


def count_tx(lines):
    return sum(1 for l in lines if "SPINE ►" in strip_ansi(l))


def world():
    tmp = tempfile.mkdtemp(prefix="obc-seqwear-")
    src = os.path.join(DATA, "world.db")
    # The brain checkpoints its WAL now and then; a copy that lands on that
    # moment is refused. Try again rather than die mid-run (2026-09-13).
    for attempt in range(10):
        try:
            for suf in ["", "-wal", "-shm"]:
                if os.path.exists(src + suf):
                    shutil.copy(src + suf, os.path.join(tmp, "world.db" + suf))
            break
        except PermissionError:
            if attempt == 9:
                raise
            time.sleep(0.5)
    c = sqlite3.connect(os.path.join(tmp, "world.db"))
    c.row_factory = sqlite3.Row
    return c


def current(c, entity):
    r = c.execute(
        "select value_json, valid_from from world_facts where entity=? order by valid_from desc, id desc limit 1",
        (entity,),
    ).fetchone()
    return (json.loads(r["value_json"]), r["valid_from"]) if r else (None, None)


def save(name, run):
    os.makedirs("results", exist_ok=True)
    out = os.path.join("results", f"{name}-{datetime.datetime.now():%Y%m%d-%H%M%S}.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(run, f, indent=2)
    return out


def step4(a):
    s = open_quiet(a.port)
    run = {"step": 4, "port": a.port, "station": a.station, "resets": a.resets, "interval_s": a.interval, "boots": []}
    print(f"[{stamp()}] resetting {a.station} on {a.port} {a.resets}x every {a.interval} s")
    s.reset_input_buffer()
    prev = None
    ok = True
    for k in range(a.resets + 1):
        reset(s)
        t0 = time.time()
        boot = wait_boot_line(s, 15)
        if boot is None:
            print(f"[{stamp()}] boot {k}: no counter line within 15 s")
            run["boots"].append({"k": k, "error": "no boot line"})
            ok = False
            break
        c, u, f, t = boot
        rec = {"k": k, "count": c, "used": u, "free": f, "total": t, "boot_ms": int((time.time() - t0) * 1000)}
        # Frames sent in the interval before this boot were counted on the previous iteration.
        if prev is not None:
            delta = c - prev["count"]
            expected = RESERVE + RESERVE * (prev["frames_between"] // RESERVE)
            rec["delta"] = delta
            rec["expected_delta"] = expected
            rec["free_delta"] = (f - prev["free"]) if (f is not None and prev["free"] is not None) else None
            rec["pass"] = delta == expected
            ok &= rec["pass"]
            print(
                f"[{stamp()}] boot {k}: count {c} (Δ{delta}, expected {expected}) nvs free {f} ({rec['free_delta']:+d} entries)"
                if rec["free_delta"] is not None
                else f"[{stamp()}] boot {k}: count {c} (Δ{delta}, expected {expected})"
            )
        else:
            print(f"[{stamp()}] boot {k}: count {c} nvs used={u} free={f} total={t}")
        # Let it run the interval, counting frames it transmits.
        lines = read_lines(s, a.interval)
        rec["frames_between"] = count_tx(lines)
        run["boots"].append(rec)
        prev = rec
    s.close()
    deltas = [b["delta"] for b in run["boots"] if "delta" in b]
    run["pass"] = ok and len(deltas) == a.resets
    run["summary"] = {
        "boots": len(run["boots"]),
        "min_delta": min(deltas) if deltas else None,
        "max_delta": max(deltas) if deltas else None,
        "writes_inferred": sum(d // RESERVE for d in deltas),
        "free_first": run["boots"][0].get("free"),
        "free_last": run["boots"][-1].get("free"),
    }
    out = save("bench_seq_wear-step4", run)
    print(f"\n{'PASS' if run['pass'] else 'FAIL'} step 4 — {out}\n{json.dumps(run['summary'])}")
    return run["pass"]


def step5(a):
    s = open_quiet(a.port)
    run = {"step": 5, "port": a.port, "station": a.station, "events": []}

    def ev(name, passed, detail):
        run["events"].append({"at": stamp(), "event": name, "pass": bool(passed), "detail": detail})
        print(f"[{stamp()}] {'PASS' if passed else 'FAIL'} {name}: {detail}")

    c = world()
    before, before_ts = current(c, f"mesh.{a.station}")
    health0, _ = current(c, f"mesh.{a.station}.health")
    esc0, _ = current(c, f"mesh.{a.station}.escalation")
    c.close()
    ev("baseline", before is not None, f"mesh.{a.station} last_type={before and before.get('last_type')} health={health0 and health0.get('status')} escalation={esc0 and esc0.get('status')}")
    if a.fault_at:
        # Resuming a run whose station-side half already passed (the fault is
        # RAM-only and the station is still silent): the brain-side half only.
        h, m, sec = (int(x) for x in a.fault_at.split(":"))
        t_fault = datetime.datetime.now().replace(hour=h, minute=m, second=sec, microsecond=0).timestamp()
        refused_at = t_fault
        ev("resumed", True, f"fault injected at {a.fault_at} in an earlier run; station-side events recorded there")
    else:
        s.reset_input_buffer()
        s.write(b'{"bench":"nvs_fault"}\n')
        s.flush()
        t_fault = time.time()
        lines = read_lines(s, 3)
        poisoned = any("NVS store poisoned" in strip_ansi(l) for l in lines)
        ev("fault injected", poisoned, "station acknowledged the poison" if poisoned else f"no acknowledgement in 3 s: {[strip_ansi(l) for l in lines][-3:]}")
        if not poisoned:
            s.close()
            save("bench_seq_wear-step5", run)
            return False

        # Watch the console until transmit is refused (the next ceiling extension).
        refused_at, tx_after = None, 0
        deadline = time.time() + a.watch
        while time.time() < deadline and refused_at is None:
            for l in read_lines(s, 2):
                l = strip_ansi(l)
                if "SPINE ►" in l:
                    tx_after += 1
                if "transmit refused" in l or "TRANSMIT DISABLED" in l:
                    refused_at = time.time()
                    break
        ev(
            "transmit refused within RESERVE frames",
            refused_at is not None and tx_after <= RESERVE,
            f"{tx_after} frames transmitted after the fault before refusal; refused after {(refused_at - t_fault):.0f} s" if refused_at else f"no refusal within {a.watch} s ({tx_after} frames sent)",
        )
        if refused_at is None:
            s.close()
            save("bench_seq_wear-step5", run)
            return False
        # Now the station must be silent: no SPINE ► lines for 60 s.
        quiet = read_lines(s, 60)
        tx_quiet = count_tx(quiet)
        ev("station silent after refusal", tx_quiet == 0, f"{tx_quiet} frames transmitted in the 60 s after refusal")

    # The brain's view: mesh.<station> stops refreshing, health offline, then escalation.
    def watch_world(pred, timeout, every=5):
        dl = time.time() + timeout
        while time.time() < dl:
            c = world()
            try:
                v = pred(c)
            finally:
                c.close()
            if v:
                return v
            time.sleep(every)
        return None

    offline = watch_world(lambda c: (lambda h: h if h and h.get("status") == "offline" else None)(current(c, f"mesh.{a.station}.health")[0]), a.watch)
    ev("brain reads the station offline (spine up, so offline — not unobservable)", offline is not None, f"health={offline} after {(time.time()-refused_at):.0f} s")
    esc = watch_world(lambda c: (lambda e: e if e and e.get("status") == "escalated" and e.get("ts_ms", 0) > t_fault * 1000 else None)(current(c, f"mesh.{a.station}.escalation")[0]), a.watch)
    ev("brain escalates the silent station", esc is not None, f"escalation={esc}")

    # Recovery: a reset clears the RAM-only fault; the counter resumes above the ceiling.
    s.reset_input_buffer()
    reset(s)
    boot = wait_boot_line(s, 15)
    ev("reset restores the counter above the persisted ceiling", boot is not None, f"boot line count={boot and boot[0]} nvs={boot and boot[1:]}")
    back = read_lines(s, 15)
    ev("station transmits again", count_tx(back) > 0, f"{count_tx(back)} frames in 15 s after the reset")
    cleared = watch_world(lambda c: (lambda e: e if e and e.get("status") == "cleared" and e.get("ts_ms", 0) > t_fault * 1000 else None)(current(c, f"mesh.{a.station}.escalation")[0]), 180)
    ev("brain clears the escalation on its return", cleared is not None, f"escalation={cleared}")
    s.close()
    run["pass"] = all(e["pass"] for e in run["events"])
    out = save("bench_seq_wear-step5", run)
    print(f"\n{'PASS' if run['pass'] else 'FAIL'} step 5 — {out}")
    return run["pass"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", required=True)
    ap.add_argument("--station", default="gw-40")
    sub = ap.add_subparsers(dest="step", required=True)
    p4 = sub.add_parser("step4")
    p4.add_argument("--resets", type=int, default=10)
    p4.add_argument("--interval", type=float, default=4.0, help="seconds the station runs between resets")
    p5 = sub.add_parser("step5")
    p5.add_argument("--watch", type=int, default=300, help="seconds to wait for each brain-side transition")
    p5.add_argument("--fault-at", default=None, help="HH:MM:SS of a fault injected by an earlier run; skip to the brain-side checks")
    a = ap.parse_args()
    ok = step4(a) if a.step == "step4" else step5(a)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
