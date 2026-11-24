#!/usr/bin/env python3
"""Plants for #134 (F1): each one alone, textually, restored in a finally;
each run of the tests under a time limit. Usage: plants_f1.py TREE OUT [ids]"""
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
REMOTE = 'crates/skyzfs-upload/src/remote.rs'
ROW = 'crates/skyzfs-upload/src/row.rs'
ENGINE = 'crates/skynbd-engine/src/lib.rs'
UP = ['-p', 'skyzfs-upload']
ENG = ['-p', 'skynbd-engine', '-p', 'skyzfs-upload']

P = [
    # The author's twelve, as the description words them.
    ('JW', 'a whole cut onto head where head\'s group is the row\'s, whatever commit the row names', CHECK,
     '            Some(commit) => commit == head,\n',
     '            Some(_) => head_group == Some(named.cut.group),\n', UP),
    ('JD', 'a delta onto head where head\'s group is the row\'s (n2)', CHECK,
     '        Some(named) if named.commit != Some(head) => Verdict::Mismatch(Mismatch::HeadMoved),\n',
     '        Some(named) if head_group != Some(named.cut.group) => Verdict::Mismatch(Mismatch::HeadMoved),\n', UP),
    ('V1D', 'a row of version 1 vouches for a delta by its group', CHECK,
     '        Some(named) if named.commit != Some(head) => Verdict::Mismatch(Mismatch::HeadMoved),\n',
     '        Some(named) if named.commit.map_or(head_group != Some(named.cut.group), |c| c != head) => Verdict::Mismatch(Mismatch::HeadMoved),\n', UP),
    ('V1W', 'under a row of version 1 no whole cut goes up', CHECK,
     '            None => head_group == Some(named.cut.group),\n',
     '            None => false,\n', UP),
    ('H1', 'a cut found committed handed on with head where head\'s group is the cut\'s (n1)', DRIVER,
     '    (named.commit == Some(seen.head)).then_some(seen.head)\n',
     '    (seen.head_group == Some(named.cut.group)).then_some(seen.head)\n', UP),
    ('H2', 'under a row of version 1, handed on with head by its group', DRIVER,
     '    (named.commit == Some(seen.head)).then_some(seen.head)\n',
     '    (named.commit == Some(seen.head) || (named.commit.is_none() && seen.head_group == Some(named.cut.group))).then_some(seen.head)\n', UP),
    ('HC', 'found committed at the check: handed on with head whatever the row names', DRIVER,
     '                    .and_then(|named| head_of(&named, &seen));\n',
     '                    .map(|_| seen.head);\n', UP),
    ('HF', 'found committed after a failed commit: handed on with head whatever the row names', DRIVER,
     '                let step = self.uploaded(&upload, head_of(&named, &seen)).await;\n',
     '                let step = { let _ = named; self.uploaded(&upload, Some(seen.head)).await };\n', UP),
    ('RE', 'the row names another commit than the one it lands with', REMOTE,
     '                value: row::encode(&cut, &commit),\n',
     '                value: row::encode(&cut, &[0u8; 32]),\n', UP),
    ('EN', 'the engine builds the ops from another hash (head\'s) than the commit\'s', ENGINE,
     '                let ops = ops(commit_hash);\n',
     '                let ops = ops(self.head.commit);\n', ENG),
    ('EO', 'ops built from the hash may swap the engine\'s own ref', ENGINE,
     '                let ops = ops(commit_hash);\n                self.own_ref_untouched(&ops)?;\n',
     '                let ops = ops(commit_hash);\n', ENG),
    ('RD', 'a row of version 1 is not read', ROW,
     '        1 => ROW_V1_LEN,\n', '', UP),
    # The review's own.
    ('R1', 'rule 1 (found committed) also asks head to be the row\'s commit', CHECK,
     '    if named.map(|named| named.cut) == Some(cut.row()) {\n',
     '    if named.map(|named| named.cut) == Some(cut.row()) && named.and_then(|named| named.commit) == Some(head) {\n', UP),
    ('R2', 'naming finds the cut by its group alone', ROW,
     '            Row::Named(named) if named.cut == *cut => Some(*named),\n',
     '            Row::Named(named) if named.cut.group == cut.group => Some(*named),\n', UP),
    ('R3', 'decode reads the commit from the wrong bytes (2..34)', ROW,
     '        _ => match bytes.get(34..66).and_then(|b| CommitHash::try_from(b).ok()) {\n',
     '        _ => match bytes.get(2..34).and_then(|b| CommitHash::try_from(b).ok()) {\n', UP),
    ('R4', 'decode of a version 2 row drops its commit (reads as version 1)', ROW,
     '        1 => None,\n        _ => match',
     '        _ => None,\n        #[allow(unreachable_patterns)]\n        _ => match', UP),
    ('R5', 'a whole cut with no row goes up whatever head\'s commit', CHECK,
     '        None => head_group.is_none(),\n', '        None => true,\n', UP),
    ('R6', 'commit_with checks its own ref only after the PUTs (the up-front check gone)', ENGINE,
     '        self.own_ref_untouched(&ops)?;\n        self.commit_inner(kind, Some(move |_| ops)).await\n',
     '        self.commit_inner(kind, Some(move |_| ops)).await\n', ENG),
    ('R7', 'the refused ops (own ref) drop the engine\'s writes', ENGINE,
     '                let ops = ops(commit_hash);\n                self.own_ref_untouched(&ops)?;\n',
     '                let ops = ops(commit_hash);\n                if let Err(e) = self.own_ref_untouched(&ops) { self.overlay = ExtentMap::new(); return Err(e); }\n', ENG),
    ('R8', 'the hand-on gives the row\'s commit whatever head is', DRIVER,
     '    (named.commit == Some(seen.head)).then_some(seen.head)\n',
     '    let _ = seen;\n    named.commit\n', UP),
    ('R9', 'a v2 row with the delta judged by group, a whole cut by commit (JD and V1D together)', CHECK,
     '        Some(named) if named.commit != Some(head) => Verdict::Mismatch(Mismatch::HeadMoved),\n',
     '        Some(named) if !head_is_the_rows => Verdict::Mismatch(Mismatch::HeadMoved),\n', UP),
]


def failing(text):
    names = set(re.findall(r'^test (\S+) \.\.\. FAILED', text, re.M))
    for blk in re.findall(r'^failures:\n((?:    \S+\n)+)', text, re.M):
        names.update(x.strip() for x in blk.splitlines())
    return sorted(names)


def run(args, limit):
    t0 = time.time()
    try:
        p = subprocess.run(['cargo', 'test', '--no-fail-fast'] + args, cwd=TREE, capture_output=True,
                           text=True, timeout=limit,
                           env=dict(os.environ, CARGO_INCREMENTAL='0', RUST_BACKTRACE='0'))
        return p.returncode, p.stdout + p.stderr, time.time() - t0
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or '')
        return 'timeout', out, time.time() - t0


res = []
for pid, what, path, old, new, crates in P:
    if ONLY and pid not in ONLY:
        continue
    full = os.path.join(TREE, path)
    orig = open(full).read()
    n = orig.count(old)
    if n != 1:
        res.append({'id': pid, 'what': what, 'verdict': f'NOT APPLIED (site found {n} times)'})
        print(pid, res[-1]['verdict'], flush=True)
        continue
    try:
        open(full, 'w').write(orig.replace(old, new))
        rc, out, secs = run(crates, 2400)
    finally:
        open(full, 'w').write(orig)
    if rc == 'timeout':
        v = 'KILLED (a hang: the tests ran out of time)'
        f = []
    elif re.search(r'^error(\[E\d+\])?:', out, re.M) and 'could not compile' in out:
        v = 'NOT APPLIED (does not compile)'
        f = []
        open(os.path.join(os.path.dirname(OUT), f'plant_{pid}.err'), 'w').write(out[-4000:])
    else:
        f = failing(out)
        v = 'KILLED' if (rc != 0 and f) else ('SURVIVED' if rc == 0 else f'rc {rc}, no failing test named')
    res.append({'id': pid, 'what': what, 'verdict': v, 'failed': f, 'secs': round(secs)})
    print(pid, v, len(f), f[:6], round(secs), flush=True)
    json.dump(res, open(OUT, 'w'), indent=1)
json.dump(res, open(OUT, 'w'), indent=1)
