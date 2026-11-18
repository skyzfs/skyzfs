import os, sys
ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import test_cut_daemon_check as t
import test_cut_daemon_main as m
k = 13
records, read, dev = t.fixture('life')
cs, _ = t.daemon_cuts_of(records, read, dev)
status = m.held_status(records, read, dev, cs).replace(f'cuts {len(cs)}\n', f'cuts {len(cs) - k}\n')
def same(written, blocks, final):
    old = {}
    for n in range(1, k + 1):
        old.update(blocks[n])
    blocks[k + 1] = dict(list(old.items())[:5])
    written[k]['blocks'] = 5
none = lambda written, blocks, final: None
for name, kw in (('the first cut of the new state holds nothing new', dict(holder=same)),
                 ('no cut of the new state written', dict(holder=none, tamper_written=lambda written, ns: [w for w in written if w.get('asked', 1) <= ns])),
                 ('a right recording', dict(holder=none))):
    rc, lines = m.recording(split=(k, 'acknowledged'), status=status, as_name='failed', **kw)
    print(f'failed, {name}: exit {rc}')
    for key, (verdict, detail) in lines.items():
        if verdict not in ('PASS', 'INFO', 'CAUGHT', 'SAME') or 'DIFFERS' in detail:
            print(f'   {key}: {verdict}: {detail[:200]}')
