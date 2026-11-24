#!/usr/bin/env python3
"""Plants for the next look at #134 (75bed3f): each plant alone, one or
more textual edits, restored in a finally; each run of the tests under a
time limit. Then repeat runs of the named tests, planted and not, for
flakiness. Usage: next_plants.py TREE OUT [ids]"""
import json
import os
import re
import subprocess
import sys
import time

TREE, OUT = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
DRIVER = 'crates/skyzfs-upload/src/driver.rs'
REMOTE = 'crates/skyzfs-upload/src/remote.rs'
CHECK = 'crates/skyzfs-upload/src/check.rs'
B1_TEST = ['--test', 'driver', 'head_swapped_while_the_engine_is_opened_again_is_judged_as_the_engine_found_it']
S2_TEST = ['--test', 'publish', 'the_upgrades_whole_cut_after_a_lost_reply_carries_its_publish']
S5_TEST = ['--test', 'publish', 'a_cut_found_committed_on_head_moved_and_moved_back_carries_its_publish']

OLD_CHECK = ('        let seen = match self.remote.observe().await {\n'
             '            Ok(seen) => seen,\n'
             '            Err(e) => return (Stage::Check, Err(e)),\n'
             '        };\n'
             '        let row = Row::of(seen.row.as_deref());\n')
AGAIN_CHECK = ('        let seen = match self.remote.observe().await {\n'
               '            Ok(seen) if !seen.current => match self.remote.observe().await {\n'
               '                Ok(seen) => seen,\n'
               '                Err(e) => return (Stage::Check, Err(e)),\n'
               '            },\n'
               '            Ok(seen) => seen,\n'
               '            Err(e) => return (Stage::Check, Err(e)),\n'
               '        };\n'
               '        let row = Row::of(seen.row.as_deref());\n')
FIXED = '            head: engine.head_commit_hash(),\n'
OLD = '            head: head.commit,\n'
HAND_ON = '                let step = self.uploaded(&upload, head_of(&named, &seen)).await;\n'

# (id, what, [(path, old, new)], repeat: (test args, runs) or None)
P = [
    ('B1', "the old line: observe returns head as first read", [(REMOTE, FIXED, OLD)], (B1_TEST, 5)),
    ('B1H', "head is the engine's, head's group is from the first read (a row of version 1 or none under the race)",
     [(REMOTE, '            head_group: engine.head_group(),\n',
       '            head_group: engine.commit_group(&head.commit).await?,\n')], None),
    ('B1R', "control, a fix of another shape: the old line, and the check observes again where not current",
     [(REMOTE, FIXED, OLD), (DRIVER, OLD_CHECK, AGAIN_CHECK)], (B1_TEST, 3)),
    ('CT', "current always true in the last Observed",
     [(REMOTE, '            current: engine.head_commit_hash() == head.commit,\n', '            current: true,\n')], None),
    ('CD', "current true in the branch of an engine left behind head with writes",
     [(REMOTE, '                    current: false,\n', '                    current: true,\n')], None),
    ('PSa', "the commit's retry hands on no commit for a cut the row names",
     [(DRIVER, HAND_ON, '                let step = { let _ = &named; self.uploaded(&upload, None).await };\n')], None),
    ('PSb', "an engine left behind head reports its own commit as head (the retry compares the row with it)",
     [(REMOTE, '                    head: head.commit,\n', '                    head: engine.head_commit_hash(),\n')], None),
    ('PSc', "the commit's retry hands on head only where the engine is current",
     [(DRIVER, HAND_ON, '                let step = self.uploaded(&upload, head_of(&named, &seen).filter(|_| seen.current)).await;\n')], None),
    ('V1W', "under a row of version 1 no whole cut goes up (the upgrade stops)",
     [(CHECK, '            None => head_group == Some(named.cut.group),\n', '            None => false,\n')], None),
]


def failing(text):
    names = set(re.findall(r'^test (\S+) \.\.\. FAILED', text, re.M))
    for blk in re.findall(r'^failures:\n((?:    \S+\n)+)', text, re.M):
        names.update(x.strip() for x in blk.splitlines())
    return sorted(names)


def run(args, limit):
    t0 = time.time()
    try:
        p = subprocess.run(['cargo', 'test', '--no-fail-fast', '-p', 'skyzfs-upload'] + args, cwd=TREE,
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


def log(line):
    print(line, flush=True)


res = []
# Unplanted, the three new tests again and again.
if not ONLY or 'base' in ONLY:
    for name, args in [('B1 test', B1_TEST), ('S2 test', S2_TEST), ('S5 test', S5_TEST)]:
        runs = 10 if name == 'B1 test' else 5
        rcs = []
        for _ in range(runs):
            rc, out, _s = run(args, 1200)
            rcs.append(rc)
        res.append({'id': f'unplanted {name}', 'runs': runs, 'passed': rcs.count(0)})
        log(f'unplanted {name}: {rcs.count(0)} of {runs} passed')
        json.dump(res, open(OUT, 'w'), indent=1)

for pid, what, edits, repeat in P:
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
            log(f'{pid} {bad}')
            continue
        rc, out, secs = run([], 2400)
        v, f = verdict(rc, out)
        if v.startswith('NOT APPLIED'):
            open(os.path.join(os.path.dirname(OUT), f'plant_{pid}.err'), 'w').write(out[-4000:])
        entry = {'id': pid, 'what': what, 'verdict': v, 'failed': f, 'secs': round(secs)}
        if repeat:
            args, runs = repeat
            rcs = [run(args, 1200)[0] for _ in range(runs)]
            entry['repeat'] = {'test': args[-1], 'runs': runs, 'failed': sum(1 for r in rcs if r != 0)}
        res.append(entry)
        log(f'{pid} {v} {len(f)} {f[:8]} {round(secs)}' + (f" repeat {entry['repeat']}" if repeat else ''))
    finally:
        for full, text in origs.items():
            open(full, 'w').write(text)
    json.dump(res, open(OUT, 'w'), indent=1)
json.dump(res, open(OUT, 'w'), indent=1)
