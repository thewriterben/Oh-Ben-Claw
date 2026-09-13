#!/usr/bin/env python3
"""SPINE-LOSS bench (OBC-Prime docs/SPINE-LOSS.md §5), against the live brain.

The brain owns the base station's port; this script only reads its world
memory (a copy of world.db + WAL each poll, so the brain's handle is never
touched) and asks the operator to pull and replug the base's USB cable. It
records what the fact history says at each step and writes the run under
results/.

    python scripts/bench_spine_loss.py [--wait 150] [--node obc-esp32-s3-001]

Steps, and what passes each:
  1. baseline   spine.gateway = open; the node has a health fact
  2. pull       within ~5 s: spine.gateway = lost/reopening with the error
  3. wait       --wait s (past escalate_after_ms): node health = unobservable,
                NO mesh.<node>.escalation written during the outage
  4. replug     within 30 s + a boot window: spine.gateway = open with the
                attempt count; then the next beacon flips the node online
"""
import argparse, datetime, json, os, shutil, sqlite3, sys, tempfile, time

DATA = os.path.join(os.environ.get("APPDATA", ""), r"thewriterben\oh-ben-claw\data")


def snapshot():
    tmp = tempfile.mkdtemp(prefix="obc-spineloss-")
    src = os.path.join(DATA, "world.db")
    for suf in ["", "-wal", "-shm"]:
        if os.path.exists(src + suf):
            shutil.copy(src + suf, os.path.join(tmp, "world.db" + suf))
    c = sqlite3.connect(os.path.join(tmp, "world.db"))
    c.row_factory = sqlite3.Row
    return c


def current(c, entity):
    r = c.execute(
        "select value_json, valid_from, valid_to from world_facts where entity=? "
        "order by valid_from desc, id desc limit 1",
        (entity,),
    ).fetchone()
    return (json.loads(r["value_json"]), r["valid_from"], r["valid_to"]) if r else (None, None, None)


def history_since(c, entity, since_ms):
    return [
        (json.loads(r["value_json"]), r["valid_from"])
        for r in c.execute(
            "select value_json, valid_from from world_facts where entity=? and valid_from>=? order by id",
            (entity, since_ms),
        )
    ]


def now_ms():
    return int(time.time() * 1000)


def stamp():
    return datetime.datetime.now().strftime("%H:%M:%S")


def wait_for(pred, timeout_s, every=1.0):
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        c = snapshot()
        try:
            v = pred(c)
        finally:
            c.close()
        if v:
            return v
        time.sleep(every)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--node", default="obc-esp32-s3-001")
    ap.add_argument("--wait", type=int, default=150, help="seconds to hold the outage (> escalate_after_ms)")
    a = ap.parse_args()
    run = {"started": datetime.datetime.now().isoformat(timespec="seconds"), "node": a.node, "steps": []}
    ok_all = True

    def step(name, passed, detail):
        nonlocal ok_all
        ok_all &= bool(passed)
        run["steps"].append({"step": name, "pass": bool(passed), "detail": detail, "at": stamp()})
        print(f"[{stamp()}] {'PASS' if passed else 'FAIL'} {name}: {detail}")

    c = snapshot()
    gw, _, _ = current(c, "spine.gateway")
    health, _, _ = current(c, f"mesh.{a.node}.health")
    esc, _, _ = current(c, f"mesh.{a.node}.escalation")
    c.close()
    step("baseline", gw is not None, f"spine.gateway={gw} health={health} escalation={esc}")
    if gw is None:
        print("the brain is not running with a supervised gateway; stop here")
        sys.exit(1)

    if gw.get("state") == "open":
        input("\n>>> PULL the base station's USB cable now, then press Enter... ")
        t_pull = now_ms()
    else:
        # The cable is already out (a restarted run): the outage began when the
        # gateway said so.
        t_pull = int(gw.get("since_ms", now_ms()))
        print(f"the link is already {gw.get('state')} since {datetime.datetime.fromtimestamp(t_pull/1000):%H:%M:%S}; continuing")
    lost = wait_for(lambda c: (lambda g: g if g and g.get("state") in ("lost", "reopening") else None)(current(c, "spine.gateway")[0]), 15)
    step("pull -> lost", lost is not None, f"spine.gateway={lost}")

    print(f"holding the outage for {a.wait} s (escalate_after_ms must be shorter)...")
    unobs = wait_for(
        lambda c: (lambda h: h if h and h.get("status") == "unobservable" else None)(current(c, f"mesh.{a.node}.health")[0]),
        a.wait,
        every=2.0,
    )
    step("node unobservable", unobs is not None, f"health={unobs}")
    remaining = a.wait - 0  # the wait above may have returned early; hold the rest
    t_end = t_pull + a.wait * 1000
    while now_ms() < t_end:
        time.sleep(2)
    c = snapshot()
    esc_during = history_since(c, f"mesh.{a.node}.escalation", t_pull)
    esc_gw40 = history_since(c, "mesh.gw-40.escalation", t_pull)
    gw_hist = history_since(c, "spine.gateway", t_pull)
    c.close()
    step(
        "no escalation during the outage",
        not esc_during and not esc_gw40,
        f"escalation facts written since pull: node={len(esc_during)} gw-40={len(esc_gw40)}; "
        f"gateway attempts so far={gw_hist[-1][0].get('attempts') if gw_hist else None}",
    )

    input("\n>>> REPLUG the base station's USB cable now, then press Enter... ")
    t_replug = now_ms()
    reopened = wait_for(lambda c: (lambda g: g if g and g.get("state") == "open" else None)(current(c, "spine.gateway")[0]), 60)
    step("replug -> open", reopened is not None, f"spine.gateway={reopened} after {(now_ms()-t_replug)/1000:.1f} s")
    online = wait_for(
        lambda c: (lambda h: h if h and h.get("status") in ("online", "degraded") else None)(current(c, f"mesh.{a.node}.health")[0]),
        90,
        every=2.0,
    )
    step("node back online on its next beacon", online is not None, f"health={online}")
    c = snapshot()
    esc_after = history_since(c, f"mesh.{a.node}.escalation", t_pull)
    gw_hist = history_since(c, "spine.gateway", t_pull)
    c.close()
    step("still no escalation", not esc_after, f"escalation facts since pull: {len(esc_after)}")
    run["gateway_history"] = [
        {"state": v.get("state"), "attempts": v.get("attempts"), "error": v.get("error"), "at": datetime.datetime.fromtimestamp(t / 1000).strftime("%H:%M:%S")}
        for v, t in gw_hist
    ]
    run["pass"] = ok_all
    os.makedirs("results", exist_ok=True)
    out = os.path.join("results", f"bench_spine_loss-{datetime.datetime.now():%Y%m%d-%H%M%S}.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(run, f, indent=2)
    print(f"\n{'PASS' if ok_all else 'FAIL'} — {out}")
    print("gateway history:", json.dumps(run["gateway_history"], indent=1))
    sys.exit(0 if ok_all else 1)


if __name__ == "__main__":
    main()
