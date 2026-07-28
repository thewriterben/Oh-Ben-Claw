import sqlite3, sys, json

c = sqlite3.connect(f"file:{sys.argv[1]}?mode=ro", uri=True)
q = """SELECT id, entity, source, origin, derived_from, substr(value_json,1,64)
       FROM world_facts WHERE valid_to IS NULL ORDER BY source, entity"""
for i, e, s, o, d, v in c.execute(q):
    print('%-16s %-9s %-34s sup=%-8s %s' % (s, o, e[:34], 'yes' if d else '-', v))
