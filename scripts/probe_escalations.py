import sqlite3, sys, time, json, collections, datetime

# Windows consoles default to cp1252, which cannot encode the arrow below and takes
# the whole script down on the print rather than on the data.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
now = time.time() * 1000

rows = list(
    c.execute(
        "SELECT id, value_json, valid_from FROM world_facts "
        "WHERE entity = 'notifications.escalation' ORDER BY valid_from"
    )
)
print(f"escalation log-of-record: {len(rows)} entries")
if rows:
    first = datetime.datetime.fromtimestamp(rows[0][2] / 1000)
    last = datetime.datetime.fromtimestamp(rows[-1][2] / 1000)
    span_h = (rows[-1][2] - rows[0][2]) / 3_600_000
    print(f"  span: {first:%m-%d %H:%M} → {last:%m-%d %H:%M}  ({span_h:.1f} h)")
    if span_h > 0:
        print(f"  rate: {len(rows)/span_h:.1f} / hour")

kinds = collections.Counter()
for _, v, _ in rows:
    try:
        r = json.loads(v).get("reason", "")
    except (ValueError, TypeError):
        r = "(unparseable)"
    kinds[r.split(".")[0][:70]] += 1
print("\n  by reason:")
for reason, n in kinds.most_common(10):
    print(f"    {n:5d}  {reason}")

print("\nwakes in the last 24 h:")
recent = [r for r in rows if now - r[2] < 86_400_000]
print(f"  {len(recent)}")

print("\nsystem2.last_wake:")
for r in c.execute(
    "SELECT value_json, valid_from FROM world_facts "
    "WHERE entity = 'system2.last_wake' ORDER BY valid_from DESC LIMIT 3"
):
    ts = datetime.datetime.fromtimestamp(r[1] / 1000)
    print(f"  {ts:%m-%d %H:%M}  {r[0][:110]}")
