#!/usr/bin/env python3
"""#96 at its next head (110d2b6): the plants of the first round that passed every test, again,
and new ones for the view following a slot by all its bytes.

    plants2.py CHECKOUT TARGET_DIR
"""
import importlib.util
import os
import sys

root, target = sys.argv[1], sys.argv[2]
sys.argv = ['plants', root, target]
spec = importlib.util.spec_from_file_location('plants', '/home/user/skyzfs/review/m2.1c/plants.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
UB, CUTS = m.UB, m.CUTS

AGAIN = ('K3 ', 'K9 ', 'K12 ', 'Z3 ')
TORN = ("                let slot_end = uberblock.at + uberblock.slot.unwrap_or(SLOT_MIN);\n"
        "                uberblock.at < end && offset < slot_end")
MORE = [
    ('R1 changed: a held slot reached past its first bytes stays held', UB,
     "        for at in torn {\n            self.held.remove(&at);\n            self.unknown.insert(at);\n        }\n", "        let _ = torn;\n"),
    ('R2 changed: an uberblock whose slot may go on leaves its place known', UB,
     "            } else if self.may_go_on(uberblock.at, end) {\n                self.unknown.insert(uberblock.at);\n            }", "            }"),
    ('R3 may_go_on: always (every unverified uberblock leaves its place unknown)', UB,
     "                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)", "                    .any(|_| true)"),
    ('R4 changed: a held slot is taken to be 1 KiB', UB, TORN,
     TORN.replace("uberblock.slot.unwrap_or(SLOT_MIN)", "SLOT_MIN")),
    ('R5 changed: a held slot is taken to be 8 KiB', UB, TORN,
     TORN.replace("uberblock.slot.unwrap_or(SLOT_MIN)", "SLOT_MAX")),
    ('R6 changed: a torn slot is dropped and its place left known', UB,
     "        for at in torn {\n            self.held.remove(&at);\n            self.unknown.insert(at);\n        }\n",
     "        for at in torn {\n            self.held.remove(&at);\n        }\n"),
    ('R7 may_go_on: a slot of any size is taken to begin anywhere', UB,
     "                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)",
     "                    .any(|size| at + size > end)"),
    ('R8 apply: an unverified uberblock marks', CUTS,
     "            if !uberblock.verified() {\n                continue;\n            }\n", ""),
    ('R9 changed: an unverified uberblock is held', UB,
     "            if uberblock.verified() {\n                self.held.insert(uberblock.at, *uberblock);",
     "            if uberblock.verified() || true {\n                self.held.insert(uberblock.at, *uberblock);"),
    ('R10 changed: every held slot after the change\'s start is torn', UB, TORN,
     TORN.replace("uberblock.at < end && offset < slot_end", "offset < slot_end")),
    ('R11 changed: a slot is torn only by a change that begins inside it', UB, TORN,
     TORN.replace("uberblock.at < end && offset < slot_end", "uberblock.at < offset && offset < slot_end")),
    ('R12 may_go_on: 8 KiB alone is asked', UB,
     "                SLOT_SIZES\n                    .into_iter()\n                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)",
     "                [SLOT_MAX]\n                    .into_iter()\n                    .any(|size| (at - ring.start).is_multiple_of(size) && at + size > end)"),
    ('R13 start: the rings read at the start go unparsed', CUTS,
     "            let found = self.found(offset, data);\n            self.view.changed(offset, data.len() as u64, &found);",
     "            let _ = self.found(offset, data);\n            self.view.changed(offset, data.len() as u64, &[]);"),
]

plants = [p for p in m.PLANTS if p[0].startswith(AGAIN)] + [p + ('UL',) if len(p) == 4 else p for p in MORE]
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
                got.append(f"{k}: {', '.join(t.split('::')[-1] for t in failed[:5])}"
                           + (f' (+{len(failed) - 5})' if len(failed) > 5 else ''))
    finally:
        open(full, 'w').write(orig)
    print(f"{name}: " + ('KILLED by ' + '; '.join(got) if got else 'SURVIVED'), flush=True)
