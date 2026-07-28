"""Measure the support-graph walk against a real store, with and without the
partial index.

Runs on a COPY. The point is not the absolute numbers — it is the ratio, and
whether a boot-time sweep over this store is milliseconds or seconds.
"""

import shutil
import sqlite3
import sys
import time
from pathlib import Path

SRC = Path(sys.argv[1])
DST = Path(sys.argv[2] if len(sys.argv) > 2 else "bench-world.db")
shutil.copy(SRC, DST)

conn = sqlite3.connect(DST)
total = conn.execute("SELECT COUNT(*) FROM world_facts").fetchone()[0]
with_support = conn.execute(
    "SELECT COUNT(*) FROM world_facts WHERE derived_from IS NOT NULL"
).fetchone()[0]
print(f"rows={total}  with_support={with_support}")

# A frontier of ids to walk from — use real ids spread through the store.
ids = [r[0] for r in conn.execute("SELECT id FROM world_facts ORDER BY id LIMIT 200")]

NAIVE = """SELECT id FROM world_facts f
           WHERE EXISTS (SELECT 1 FROM json_each(f.derived_from) WHERE json_each.value = ?)"""
GUARDED = """SELECT id FROM world_facts f
             WHERE f.derived_from IS NOT NULL AND EXISTS
                   (SELECT 1 FROM json_each(f.derived_from) WHERE json_each.value = ?)"""


def timeit(sql, label):
    t = time.perf_counter()
    n = 0
    for i in ids:
        n += len(conn.execute(sql, (i,)).fetchall())
    dt = time.perf_counter() - t
    print(f"  {label:34s} {dt*1000:8.1f} ms for {len(ids)} walk steps "
          f"({dt/len(ids)*1000:6.3f} ms each), {n} hits")
    return dt


conn.execute("DROP INDEX IF EXISTS idx_world_support")
print("no index:")
a = timeit(NAIVE, "naive (no NULL guard)")
b = timeit(GUARDED, "guarded, index absent")

conn.execute(
    "CREATE INDEX IF NOT EXISTS idx_world_support ON world_facts(id) "
    "WHERE derived_from IS NOT NULL"
)
print("with partial index:")
c = timeit(GUARDED, "guarded + partial index")

print(f"\nspeedup vs naive: {a/c:.1f}x")
print("query plan:")
for r in conn.execute("EXPLAIN QUERY PLAN " + GUARDED, (1,)):
    print("   ", r[-1])
