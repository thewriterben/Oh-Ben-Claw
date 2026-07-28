import sqlite3, sys, time

db = sys.argv[1]
c = sqlite3.connect(db)
print('cols:', [r[1] for r in c.execute("PRAGMA table_info(world_facts)")])
print('total rows:', c.execute('SELECT COUNT(*) FROM world_facts').fetchone()[0])
print('open rows :', c.execute('SELECT COUNT(*) FROM world_facts WHERE valid_to IS NULL').fetchone()[0])
print()
print('open facts by origin/source:')
for r in c.execute('SELECT origin, source, COUNT(*) FROM world_facts WHERE valid_to IS NULL GROUP BY origin, source ORDER BY 3 DESC'):
    print('   %-10s %-18s %d' % r)
print()
print('open mesh.* facts:')
q = ("SELECT id, entity, substr(value_json,1,60), origin, source, ingested_at "
     "FROM world_facts WHERE valid_to IS NULL AND entity LIKE 'mesh%' ORDER BY id")
now = time.time() * 1000
for r in c.execute(q):
    age_h = (now - r[5]) / 3_600_000 if r[5] else 0
    print('   #%-5d %-28s %-40s %-9s %-16s %.1fh old' % (r[0], r[1], r[2], r[3], r[4], age_h))
