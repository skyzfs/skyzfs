#!/usr/bin/env python3
"""Plants on node B's snap-row at c7cc03f (the next look at #139, N2):
the four that survived the pull request's tests at cabd5a0 (SR11, SR14,
SR15, SR16), at the head's text, and two that were killed then (SR1,
SR17) as a check that the run sees a failure.

Each plant replaces text that must occur exactly once in the file (else
NOT APPLIED), runs `cargo test -p skyzfs-nodeb --no-fail-fast` (2400 s
limit) and puts the file back in a finally. KILLED: a test failed or the
build failed; SURVIVED: every test passed. CARGO_TARGET_DIR is taken from
the environment. Usage: next_plants_rs.py WORKTREE OUT.json [ID...]
"""
import json
import os
import re
import subprocess
import sys

NB = 'bins/skyzfs-nodeb/src/main.rs'

PLANTS = [
    ('SR1', 'the state read one byte late', [('hex(&b[2..18]),', 'hex(&b[3..19]),')]),
    ('SR11', 'the version read by its low byte alone', [('    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {', '    if b.len() != SNAP_ROW_LEN || b[0] != 1 {')]),
    ('SR14', 'the row read at the key snap/T (not skyzfs/snap/T)', [('    let key = format!("{SNAP_ROW_PREFIX}{snapshot}");', '    let key = format!("snap/{snapshot}");')]),
    ('SR15', 'the ref read as snaps/T', [('    let name = format!("snap/{snapshot}");', '    let name = format!("snaps/{snapshot}");')]),
    ('SR16', 'the manifest fetched from skyzfs/manifests/', [('        "{}skyzfs/manifest/{}",\n        skynbd_engine::keys::volume_prefix(vol),\n        r.snap\n    );\n    match store', '        "{}skyzfs/manifests/{}",\n        skynbd_engine::keys::volume_prefix(vol),\n        r.snap\n    );\n    match store')]),
    ('SR17', 'the prefix constant skyzfs/snaps/', [('const SNAP_ROW_PREFIX: &str = "skyzfs/snap/";', 'const SNAP_ROW_PREFIX: &str = "skyzfs/snaps/";')]),
]


def run_tests(wt):
    try:
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-nodeb', '--no-fail-fast'], cwd=wt, capture_output=True,
                           text=True, timeout=2400)
        rc, out = p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return 'timeout', [], ''
    failed = re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)
    passed = len(re.findall(r'^test \S+ \.\.\. ok', out, re.M))
    build = 'error[' in out or 'could not compile' in out
    note = 'build failed: ' + ' | '.join(re.findall(r'^error.*$', out, re.M)[:3]) if build else f'{passed} passed'
    return rc, failed, note


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    rc, failed, note = run_tests(wt)
    print('baseline', rc, failed, note, flush=True)
    if rc != 0:
        sys.exit(2)
    results = []
    for pid, what, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, NB)
        with open(path) as f:
            orig = f.read()
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:50]!r})'
                break
            text = text.replace(old, new)
        res = {'id': pid, 'what': what, 'file': NB}
        if status is None:
            try:
                with open(path, 'w') as f:
                    f.write(text)
                rc, failed, note = run_tests(wt)
                res.update(result='KILLED' if rc != 0 else 'SURVIVED', failed=failed, note=note)
            finally:
                with open(path, 'w') as f:
                    f.write(orig)
        else:
            res.update(result=status)
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        res['clean_after'] = clean.strip() == ''
        results.append(res)
        print(pid, res.get('result'), res.get('failed'), res.get('note'), flush=True)
        with open(outp, 'w') as f:
            json.dump(results, f, indent=1)


if __name__ == '__main__':
    main()
