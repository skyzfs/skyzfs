#!/usr/bin/env python3
"""Plants for the review of #138 (fd8f685): each plant alone, one or more
textual edits, each site found exactly once, restored in a finally; the
crate's whole suite under a time limit (`cargo test --no-fail-fast -p
skyzfs-upload`), then the two race tests again and again. Usage:
plants.py TREE OUT [ids]"""
import json
import os
import re
import subprocess
import sys
import time

TREE, OUT = sys.argv[1], sys.argv[2]
ONLY = set(sys.argv[3:])
REMOTE = 'crates/skyzfs-upload/src/remote.rs'
DRIVER = 'crates/skyzfs-upload/src/driver.rs'
CHECK = 'crates/skyzfs-upload/src/check.rs'
NEW = 'head_swapped_before_the_first_cut_is_judged_by_the_group_the_engine_found'
B1T = 'head_swapped_while_the_engine_is_opened_again_is_judged_as_the_engine_found_it'

HEAD_FIXED = '            head: engine.head_commit_hash(),\n'
HEAD_OLD = '            head: head.commit,\n'
GROUP_FIXED = '            head_group: engine.head_group(),\n'
GROUP_FIRST = '            head_group: engine.commit_group(&head.commit).await?,\n'
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
JUDGE = '        match judge(&cut, &row, seen.head, seen.head_group) {\n'

# (id, what, [(path, old, new)])
P = [
    ('B1H', "L1's plant: head is the engine's, head's group from observe's first read of head",
     [(REMOTE, GROUP_FIXED, GROUP_FIRST)]),
    ('B1', "B1's original line: observe returns head as first read (head's group the engine's)",
     [(REMOTE, HEAD_FIXED, HEAD_OLD)]),
    ('B1B1H', "both: head and head's group from observe's first read",
     [(REMOTE, HEAD_FIXED, HEAD_OLD), (REMOTE, GROUP_FIXED, GROUP_FIRST)]),
    ('B1RH', "control, a correct fix of another shape: head and head's group from the first read, and the check observes again where not current",
     [(REMOTE, HEAD_FIXED, HEAD_OLD), (REMOTE, GROUP_FIXED, GROUP_FIRST), (DRIVER, OLD_CHECK, AGAIN_CHECK)]),
    ('NR', "with no row a whole cut goes up whatever head's group (the rule itself)",
     [(CHECK, '        None => head_group.is_none(),\n', '        None => true,\n')]),
    ('NC', "stricter: with no row the check refuses wherever the engine was opened on another head than observe's first read",
     [(DRIVER, JUDGE,
       '        match if !seen.current && matches!(row, Row::Absent) { Verdict::Mismatch(Mismatch::HeadMoved) } else { judge(&cut, &row, seen.head, seen.head_group) } {\n')]),
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
                           env=dict(os.environ, CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0',
                                    CARGO_PROFILE_TEST_DEBUG='0', RUST_BACKTRACE='0'))
        return p.returncode, p.stdout + p.stderr, time.time() - t0
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or '')
        return 'timeout', out, time.time() - t0


def verdict(rc, out):
    if rc == 'timeout':
        return 'KILLED (a hang: the tests ran out of time)', []
    if 'could not compile' in out:
        return 'NOT APPLIED (does not compile)', []
    f = failing(out)
    return ('KILLED' if (rc != 0 and f) else ('SURVIVED' if rc == 0 else f'rc {rc}, no failing test named')), f


def counts(out):
    p = f = 0
    for m in re.finditer(r'^test result: \w+\. (\d+) passed; (\d+) failed', out, re.M):
        p += int(m.group(1)); f += int(m.group(2))
    return p, f


def log(line):
    print(line, flush=True)


def first_panic(out, name):
    m = re.search(r"thread '" + name + r"'.*?panicked at (\S+):\n(.*?)\n(?:note:|\n)", out, re.S)
    return (m.group(1) + ' ' + ' | '.join(m.group(2).splitlines()[:3])) if m else None


res = []
if not ONLY or 'base' in ONLY:
    rc, out, secs = run([], 2400)
    p, f = counts(out)
    res.append({'id': 'unplanted', 'rc': rc, 'passed': p, 'failed': f, 'secs': round(secs)})
    log(f'unplanted suite: rc {rc}, {p} passed, {f} failed, {round(secs)} s')
    json.dump(res, open(OUT, 'w'), indent=1)

for pid, what, edits in P:
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
        if v.startswith('NOT APPLIED') or v.startswith('rc '):
            open(os.path.join(os.path.dirname(OUT), f'plant_{pid}.err'), 'w').write(out[-6000:])
        entry = {'id': pid, 'what': what, 'verdict': v, 'failed': f, 'secs': round(secs),
                 'new_test': 'FAILED' if any(NEW in x for x in f) else 'passed',
                 'b1_test': 'FAILED' if any(B1T in x for x in f) else 'passed',
                 'new_panic': first_panic(out, NEW), 'b1_panic': first_panic(out, B1T)}
        if not v.startswith('NOT APPLIED'):
            rep = {}
            for name in (NEW, B1T):
                rcs = [run(['--test', 'driver', name, '--', '--exact'], 1200)[0] for _ in range(5)]
                rep[name] = f'{sum(1 for r in rcs if r != 0)} of 5 failed'
            entry['repeat'] = rep
        res.append(entry)
        log(f"{pid} {v} {len(f)} {f[:8]} {round(secs)} s; new test {entry['new_test']}, B1's test {entry['b1_test']}; repeat {entry.get('repeat')}")
    finally:
        for full, text in origs.items():
            open(full, 'w').write(text)
    json.dump(res, open(OUT, 'w'), indent=1)
json.dump(res, open(OUT, 'w'), indent=1)
