import os, sys
ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import test_cut_daemon_check as t
import test_cut_daemon_main as m
none = lambda written, blocks, final: None
records, read, dev = t.fixture('life')
cs, _ = t.daemon_cuts_of(records, read, dev)
K = len(cs) // 2
status = m.held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
for kind in ('taken-over', 'added'):
    for as_name in ('takeover', None):
        try:
            rc, lines = m.recording(holder=none, split=(K, kind), status=status, as_name=as_name)
        except Exception as e:
            print(kind, as_name, 'raised', type(e).__name__, str(e)[:200]); continue
        print(f'{kind} as {as_name}: exit {rc}')
        for key, (verdict, detail) in lines.items():
            if verdict not in ('PASS', 'INFO', 'CAUGHT', 'SAME') or 'DIFFERS' in detail:
                print(f'   {key}: {verdict}: {detail[:260]}')
