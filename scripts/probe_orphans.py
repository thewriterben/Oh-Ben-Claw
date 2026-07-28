import sqlite3, sys, datetime

c = sqlite3.connect(sys.argv[1])
q = """SELECT id, entity, value_json, ingested_at, origin, source
       FROM world_facts WHERE valid_to IS NULL
         AND (entity LIKE 'mesh.escalation_status%'
              OR entity IN ('mesh.escalated_count','mesh.gw-40.failure',
                            'mesh.gw-40.escalation','mesh.obc-esp32-s3-001.escalation'))
       ORDER BY id"""
for i, e, v, ing, o, s in c.execute(q):
    ts = datetime.datetime.fromtimestamp(ing / 1000).strftime('%m-%d %H:%M')
    print('#%-5d %-36s %-9s %-16s %s' % (i, e, o, s, ts))
    print('        %s' % v[:150])
