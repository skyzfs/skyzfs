#!/usr/bin/env python3
"""Write the next look's plant specs: the survivors of the review of
#159 and of #158's sets, adapted to the head (WT), plus new plants."""
import sys
wt, rev = sys.argv[1], sys.argv[2]   # worktree at the head; the repo's review/ dir
def load(path):
    ns = {}
    exec(open(path).read(), ns)
    return ns['PLANTS']
def pick(plants, ids):
    return [p for p in plants if p[0].split()[0] in ids]
def adapt(p):
    pid, fn, old, new, cmd = p
    text = open(f'{wt}/{fn}').read()
    if text.count(old) == 1:
        return p
    ind = lambda s: '\n'.join(('    ' + ln) if ln.startswith(' ' * 12) else ln for ln in s.split('\n'))
    o2, n2 = ind(old), ind(new)
    if text.count(o2) == 1:
        return (pid + ' (re-indented)', fn, o2, n2, cmd)
    return p
out = []
a = pick(load(f'{rev}/m2.t5/pr158/next_plants_a.py'),
         {'P43', 'P46', 'P48', 'P52', 'P60', 'P61', 'P64', 'P65', 'P67', 'P73', 'P74', 'Q06', 'Q23'})
b = pick(load(f'{rev}/m2.t5/pr158/next_plants_b.py'), {'N05', 'N07'})
c = pick(load(f'{rev}/m2.t5/pr158/next_plants_c.py'), {'S03', 'S05', 'S15', 'S16', 'S17', 'S18', 'S22', 'S23'})
d = pick(load(f'{rev}/m2.t5/pr158/next_plants_d.py'), {'T09'})
k = pick(load(f'{rev}/m2.t5b/pr159/new_py.py'), {'K08', 'K21', 'K22', 'K24'})
u = pick(load(f'{rev}/m2.t5b/pr159/new_sh.py'),
         {'U05', 'U11', 'U12', 'U13', 'U14', 'U15', 'U16', 'U18', 'U19', 'U20', 'U21', 'U30', 'V01', 'V02', 'V05', 'S21a'})
r = pick(load(f'{rev}/m2.t5b/pr159/retain_b.py') + load(f'{rev}/m2.t5b/pr159/retain.py'),
         {'R14', 'A04', 'A07', 'A08', 'A09'})
seen = set(); r = [p for p in r if not (p[0] in seen or seen.add(p[0]))]
for name, ps in (('py', a + b + k), ('sh', c + d + u), ('r', r)):
    with open(f'old_{name}.py', 'w') as f:
        f.write('# Survivors re-run at the head, anchors adapted where the code moved.\nPLANTS = [\n')
        for p in ps:
            f.write(f' {adapt(p)!r},\n')
        f.write(']\n')
    print(name, len(ps))
