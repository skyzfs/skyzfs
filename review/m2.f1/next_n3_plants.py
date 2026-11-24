#!/usr/bin/env python3
"""Plants for N3 (7989f5e, `row_v1`), the review's next look at #134. RV1, RVG, RVD and RVC are
the author's plants run again (their ids as in the description); QN, QX, QW, QA and
QU are the review's own. Each plant alone, one or more
textual edits, restored in a finally; the named crate's tests under a time
limit. Usage: next_n3_plants.py TREE OUT [ids]"""
import json
import os
import re
import subprocess
import sys
import time

TREE, OUT = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
CHECK = 'crates/skyzfs-upload/src/check.rs'
DRIVER = 'crates/skyzfs-upload/src/driver.rs'
DAEMON = 'bins/skyzfsd/src/upload.rs'
STATUS = 'crates/skyzfs-ublk/src/status.rs'
UPLOAD = ['-p', 'skyzfs-upload']

GUARD = ('        // A row of version 1 whose group is head\'s: only the commit it\n'
         '        // does not name stands between the cut and a delta (N3).\n'
         '        Some(named) if named.commit.is_none() && head_is_the_rows => {\n'
         '            Verdict::Mismatch(Mismatch::RowV1)\n'
         '        }\n')
NOT_BASE = '        Some(named) if Some(named.cut) != cut.base => Verdict::Mismatch(Mismatch::NotTheBase),\n'

# (id, what, [(path, old, new)], cargo args)
P = [
    ('RV1', "author's: the upgrade's first delta is HeadMoved again",
     [(CHECK, '            Verdict::Mismatch(Mismatch::RowV1)\n', '            Verdict::Mismatch(Mismatch::HeadMoved)\n')], UPLOAD),
    ('RVG', "author's: RowV1 for every version 1 row naming the base, whatever head's group",
     [(CHECK, 'Some(named) if named.commit.is_none() && head_is_the_rows => {',
       'Some(named) if named.commit.is_none() => {')], UPLOAD),
    ('QN', "the RowV1 guard before NotTheBase: a version 1 row naming another cut reads row_v1",
     [(CHECK, NOT_BASE + GUARD, GUARD + NOT_BASE)], UPLOAD),
    ('QX', "RowV1 also for a version 2 row where head is another commit of the row's group (hides n2's move)",
     [(CHECK, 'Some(named) if named.commit.is_none() && head_is_the_rows => {',
       'Some(named) if (named.commit.is_none() && head_is_the_rows) || head_group == Some(named.cut.group) && named.commit != Some(head) => {')],
     UPLOAD),
    ('QW', "a whole cut under a version 1 row with head at another group is RowV1, not HeadMoved",
     [(CHECK, '        return if head_is_the_rows {\n            Verdict::Upload(Rule::Whole)\n        } else {\n            Verdict::Mismatch(Mismatch::HeadMoved)\n        };\n',
       '        return if head_is_the_rows {\n            Verdict::Upload(Rule::Whole)\n        } else if named.is_some_and(|n| n.commit.is_none()) {\n            Verdict::Mismatch(Mismatch::RowV1)\n        } else {\n            Verdict::Mismatch(Mismatch::HeadMoved)\n        };\n')],
     UPLOAD),
    ('QA', "the driver does not abandon the held cuts at RowV1",
     [(DRIVER, '                let abandoned = self.local.abandon_all().await;\n',
       '                let abandoned = if why == Mismatch::RowV1 { Default::default() } else { self.local.abandon_all().await };\n')],
     UPLOAD),
    ('RVD', "author's: the daemon shows RowV1 as head_moved",
     [(DAEMON, 'Checked::Mismatched(Mismatch::RowV1) => CheckedShown::RowV1,',
       'Checked::Mismatched(Mismatch::RowV1) => CheckedShown::HeadMoved,')], ['-p', 'skyzfsd', '--lib']),
    ('RVC', "author's: row_v1 is written as code 7",
     [(STATUS, 'RowV1 = 6, "row_v1";', 'RowV1 = 7, "row_v1";')], ['-p', 'skyzfs-ublk', '--lib']),
    ('QU', "row_v1 is written as code 5, the code of unreadable (a reader cannot tell them apart)",
     [(STATUS, 'RowV1 = 6, "row_v1";', 'RowV1 = 5, "row_v1";')], ['-p', 'skyzfs-ublk', '--lib']),
]


def failing(text):
    names = set(re.findall(r'^test (\S+) \.\.\. FAILED', text, re.M))
    for blk in re.findall(r'^failures:\n((?:    \S+\n)+)', text, re.M):
        names.update(x.strip() for x in blk.splitlines())
    return sorted(names)


def run(args, limit):
    t0 = time.time()
    try:
        p = subprocess.run(['cargo', 'test', '--no-fail-fast'] + args, cwd=TREE,
                           capture_output=True, text=True, timeout=limit,
                           env=dict(os.environ, CARGO_INCREMENTAL='0', RUST_BACKTRACE='0'))
        return p.returncode, p.stdout + p.stderr, time.time() - t0
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or '')
        return 'timeout', out, time.time() - t0


def verdict(rc, out):
    if rc == 'timeout':
        return 'KILLED (a hang: the tests ran out of time)', []
    if re.search(r'^error(\[E\d+\])?:', out, re.M) and 'could not compile' in out:
        return 'NOT APPLIED (does not compile)', []
    f = failing(out)
    return ('KILLED' if (rc != 0 and f) else ('SURVIVED' if rc == 0 else f'rc {rc}, no failing test named')), f


res = []
for pid, what, edits, args in P:
    if ONLY and pid not in ONLY:
        continue
    origs = {}
    try:
        bad = None
        for path, old, new in edits:
            full = os.path.join(TREE, path)
            if full not in origs:
                origs[full] = open(full).read()
            text = open(full).read()
            n = text.count(old)
            if n != 1:
                bad = f'NOT APPLIED (site found {n} times in {path})'
                break
            open(full, 'w').write(text.replace(old, new))
        if bad:
            res.append({'id': pid, 'what': what, 'verdict': bad})
            print(f'{pid} {bad}', flush=True)
            continue
        rc, out, secs = run(args, 2400)
        v, f = verdict(rc, out)
        if v.startswith('NOT APPLIED') or v.startswith('rc '):
            open(os.path.join(os.path.dirname(OUT), f'next_n3_plant_{pid}.err'), 'w').write(out[-4000:])
        res.append({'id': pid, 'what': what, 'args': ' '.join(args), 'verdict': v, 'failed': f, 'secs': round(secs)})
        print(f'{pid} {v} {len(f)} {f[:6]} {round(secs)}', flush=True)
    finally:
        for full, text in origs.items():
            open(full, 'w').write(text)
    json.dump(res, open(OUT, 'w'), indent=1)
json.dump(res, open(OUT, 'w'), indent=1)
