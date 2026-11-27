import os, sys, shutil, tempfile, importlib
src = sys.argv[1]
PLANTS = {
 'S released may be before': ("rel < r1[0]['round'] <= rel + 2 else", "0 < r1[0]['round'] <= rel + 2 else"),
 'S released +3': ("rel < r1[0]['round'] <= rel + 2 else", "rel < r1[0]['round'] <= rel + 3 else"),
 'judge_kept no retire check': ("if p.get('retire') != '-':", "if False:"),
 'retired_once ignores duplicates': ("return [] if seen == list(want) else", "return [] if list(dict.fromkeys(seen)) == list(want) else"),
 'retired_once ignores order': ("return [] if seen == list(want) else", "return [] if sorted(seen) == sorted(want) else"),
}
def scen_released_while_held(t, m):
    t.set_round(m, 'S', 'publish', 4, [('retired', 's2'), ('retired', 's1'), ('snap', 's4')])
    t.set_round(m, 'S', 'publish', 5, [('retired', 's3'), ('snap', 's5')])
def scen_released_late(t, m):
    t.set_round(m, 'S', 'publish', 5, [('retired', 's3'), ('snap', 's5')])
    t.set_round(m, 'S', 'publish', 6, [('retired', 's4'), ('snap', 's6')])
    m['S']['runs']['publish'].append([('retired', 's1'), ('retired', 's5'), ('snap', 's7')])
    m['S']['kept'], m['S']['retired'] = ['s6', 's7'], ['s1', 's2', 's3', 's4', 's5']
    m['S']['retires'] = [('s1', 'retired'), ('s2', 'retired'), ('s3', 'retired'), ('s4', 'retired'), ('s5', 'retired')]
def scen_kept_marked(t, m):
    m['B']['files']['record-after.txt'] = ''.join(
        f'published {x} commit={t.commit_of(x)} group=9 members=4 at_ms=3 retire={r}\n' for x, r in (('b12', '-'), ('b13', 'ref')))
def scen_b_said_twice(t, m):
    t.set_round(m, 'B', 'run2', 3, [('retired', 'b11'), ('retired', 'b11'), ('snap', 'b13')])
def scen_b_order_in_round(t, m):
    bs = [f'b{i}' for i in range(1, 14)]
    t.set_round(m, 'B', 'run2', 1, [('retired', x) for x in [bs[1], bs[0]] + bs[2:8]] + [('snap', bs[10])])
CASES = [('S released while held', scen_released_while_held, 'pin_S_released', 'S released may be before'),
         ('S released three rounds after', scen_released_late, 'pin_S_released', 'S released +3'),
         ('B kept name marked ref only', scen_kept_marked, 'pin_B_kept', 'judge_kept no retire check'),
         ('B retired said twice', scen_b_said_twice, 'pin_B_retired', 'retired_once ignores duplicates'),
         ('B retired out of order in a round', scen_b_order_in_round, 'pin_B_retired', 'retired_once ignores order')]
for label, scen, key, plant in CASES:
    out = []
    for planted in (False, True):
        d = tempfile.mkdtemp(prefix='iso')
        k = os.path.join(d, 'k'); shutil.copytree(src, k)
        if planted:
            p = os.path.join(k, 'agent_retain_check.py'); s = open(p).read()
            old, new = PLANTS[plant]; assert s.count(old) == 1; open(p, 'w').write(s.replace(old, new))
        sys.path.insert(0, k)
        for mod in ('agent_retain_check', 'test_agent_retain_check'):
            sys.modules.pop(mod, None)
        t = importlib.import_module('test_agent_retain_check')
        st, v, det = t.run(lambda m: scen(t, m))
        out.append(f"{'planted' if planted else 'original'}: {key}={v.get(key)} exit={st}")
        sys.path.remove(k); shutil.rmtree(d)
    print(f'{label}: ' + '; '.join(out))
