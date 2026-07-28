import sqlite3, sys, json, collections, datetime

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)

print("vision.subject.* rows written (all history):")
rows = list(
    c.execute(
        "SELECT id, entity, value_json, ingested_at FROM world_facts "
        "WHERE entity LIKE 'vision.subject.%' ORDER BY id"
    )
)
print(f"  {len(rows)} rows")

events = collections.Counter()
for _, _, v, _ in rows:
    try:
        events[json.loads(v).get("event_id")] += 1
    except (ValueError, TypeError):
        events["(unparseable)"] += 1
print(f"  distinct event_ids: {len(events)}")
print(f"  most repeated:")
for ev, n in events.most_common(5):
    print(f"    {n:5d}x  {ev}")

print("\nvision.count.* current values and write counts:")
for r in c.execute(
    "SELECT entity, COUNT(*) FROM world_facts WHERE entity LIKE 'vision.count.%' "
    "GROUP BY entity ORDER BY 2 DESC"
):
    cur = c.execute(
        "SELECT value_json FROM world_facts WHERE entity = ? AND valid_to IS NULL", (r[0],)
    ).fetchone()
    print(f"  {r[0]:<34} writes={r[1]:<6} current={cur[0] if cur else '-'}")

print("\ndetection timestamps — are new events actually arriving?")
for r in c.execute(
    "SELECT MIN(valid_from), MAX(valid_from) FROM world_facts WHERE entity LIKE 'vision.subject.%'"
):
    if r[0]:
        a = datetime.datetime.fromtimestamp(r[0] / 1000)
        b = datetime.datetime.fromtimestamp(r[1] / 1000)
        print(f"  detection valid_from spans {a:%Y-%m-%d %H:%M} .. {b:%Y-%m-%d %H:%M}")
for r in c.execute(
    "SELECT MIN(ingested_at), MAX(ingested_at) FROM world_facts WHERE entity LIKE 'vision.subject.%'"
):
    if r[0]:
        a = datetime.datetime.fromtimestamp(r[0] / 1000)
        b = datetime.datetime.fromtimestamp(r[1] / 1000)
        print(f"  ingested_at      spans {a:%Y-%m-%d %H:%M} .. {b:%Y-%m-%d %H:%M}")
