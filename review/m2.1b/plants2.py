#!/usr/bin/env python3
"""The plants of review/m2.1b/plants.py that passed every test at d665a95, again at the next head,
with two more for what that head adds.

    plants2.py CHECKOUT TARGET_DIR
"""
import importlib.util
import os
import sys

root, target = sys.argv[1], sys.argv[2]
sys.argv = ['plants', root, target]
spec = importlib.util.spec_from_file_location('plants', '/home/user/skyzfs/review/m2.1b/plants.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

SERVE = 'bins/skyzfsd/src/serve.rs'
AGAIN = ('V1 ', 'V13 ', 'C4 ', 'C9 ', 'T1 ', 'T2 ', 'T3 ')
# C4 and C6 of the first round matched read_rings' `if read.is_ok()`, which is now a match.
REPLACED = {
    'C4 ': ('C4 start: a ring that cannot be read is taken as read', m.INPLACE,
            "                Err(e) => self.rings_unread.push((offset, e.to_string())),",
            "                Err(_) => self.cuts.read_at_start(offset, &buf),", 'L'),
}
MORE = [
    ('N1 start: a ring that cannot be read is not noted', m.INPLACE,
     "                Err(e) => self.rings_unread.push((offset, e.to_string())),",
     "                Err(e) => drop(e),", 'L'),
    ('N2 serve: the start does not say which rings it could not read', SERVE,
     "    if let Some(said) = rings_unread_said(served.rings_unread()) {\n        say(said);\n    }\n", "", 'S'),
    ('N3 serve: the line names no ring when one could not be read', SERVE,
     "    let (first, why) = unread.first()?;", "    let (first, why) = unread.get(1)?;", 'S'),
]

plants = []
for p in m.PLANTS:
    for a in AGAIN:
        if p[0].startswith(a):
            plants.append(REPLACED.get(a, p))
plants += MORE

for name, path, old, new, kinds in plants:
    full = os.path.join(root, path)
    orig = open(full).read()
    n = orig.count(old)
    if n != 1:
        print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
        continue
    open(full, 'w').write(orig.replace(old, new))
    try:
        got = []
        for k in kinds:
            rc, failed = m.run(k)
            if rc:
                got.append(f"{k}: {', '.join(t.split('::')[-1] for t in failed[:5])}")
    finally:
        open(full, 'w').write(orig)
    print(f"{name}: " + ('KILLED by ' + '; '.join(got) if got else 'SURVIVED'), flush=True)
