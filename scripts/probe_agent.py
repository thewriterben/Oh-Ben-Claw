import sqlite3, sys, time

c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
now = time.time() * 1000
print("closed_reason on the rows the retention sweep touched:")
for r in c.execute(
    "SELECT id, entity, closed_reason, valid_to FROM world_facts "
    "WHERE closed_reason IS NOT NULL AND closed_reason LIKE 'expired%' ORDER BY id"
):
    print("   #%-6d %-40s %s" % (r[0], r[1], r[2]))
print()
print("agent facts still open, with age:")
for r in c.execute(
    "SELECT id, entity, ingested_at FROM world_facts "
    "WHERE valid_to IS NULL AND source = 'agent' ORDER BY id"
):
    print("   #%-6d %-40s age %.2f days" % (r[0], r[1], (now - r[2]) / 86400000))
