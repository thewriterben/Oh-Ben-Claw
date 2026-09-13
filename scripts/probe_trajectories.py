#!/usr/bin/env python3
"""probe_trajectories.py — what is actually in a trajectories.db, before believing anything about it.

    python scripts/probe_trajectories.py "%APPDATA%\thewriterben\oh-ben-claw\data\trajectories.db"

Prints the tables, the episode count by outcome, how many carry an embedding
(the mushroom body only sees those), the date span, and the twenty most
recent objectives. Read-only.
"""

import sqlite3
import sys
from datetime import datetime, timezone


def main() -> int:
    path = sys.argv[1]
    c = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    tables = [r[0] for r in c.execute("select name from sqlite_master where type='table'")]
    print("tables:", tables)
    for t in tables:
        cols = [r[1] for r in c.execute(f"pragma table_info({t})")]
        n = c.execute(f"select count(*) from {t}").fetchone()[0]
        print(f"  {t}: {n} rows, columns {cols}")
    if "episodes" not in tables:
        return 0
    cols = [r[1] for r in c.execute("pragma table_info(episodes)")]
    print("by outcome:", c.execute("select outcome, count(*) from episodes group by outcome").fetchall())
    ts_col = next((x for x in cols if x in ("ts_ms", "created_ms", "timestamp_ms", "started_ms")), None)
    if ts_col:
        lo, hi = c.execute(f"select min({ts_col}), max({ts_col}) from episodes").fetchone()
        f = lambda ms: datetime.fromtimestamp(ms / 1000, tz=timezone.utc).isoformat() if ms else None  # noqa: E731
        print(f"span: {f(lo)} → {f(hi)}")
    emb_tables = [t for t in tables if "vec" in t or "embed" in t]
    for t in emb_tables:
        print(f"embedded rows in {t}:", c.execute(f"select count(*) from {t}").fetchone()[0])
    order = ts_col or "rowid"
    print("most recent objectives:")
    for row in c.execute(f"select objective, outcome from episodes order by {order} desc limit 20"):
        print(f"  [{row[1]}] {str(row[0])[:100]!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
