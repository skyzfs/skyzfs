#!/usr/bin/env python3
"""Plants on node B's snap-row decode, its manifest check, and the guard's
call site in move-head (the review of #139, M2.T7, at cabd5a0).

Each plant replaces text that must occur exactly once in its file (else
NOT APPLIED), runs `cargo test -p skyzfs-nodeb --no-fail-fast` (2400 s
limit) on the pull request's tests alone, and then, for the decode's
plants, with the review's cross-check (xcheck_snaprow.rs) appended; the
file is put back in a finally. KILLED: a test failed or the build failed;
SURVIVED: every test passed. Usage: plants_rs.py WORKTREE OUT.json [ID...]
"""
import json
import os
import re
import subprocess
import sys

NB = 'bins/skyzfs-nodeb/src/main.rs'
HERE = os.path.dirname(os.path.abspath(__file__))
XCHECK = open(os.path.join(HERE, 'xcheck_snaprow.rs')).read()

PLANTS = [
    ('SR1', "the state read one byte late", [('hex(&b[2..18]),', 'hex(&b[3..19]),')]),
    ('SR2', 'number and group swapped', [('        u64_at(18),\n        u64_at(26),', '        u64_at(26),\n        u64_at(18),')]),
    ('SR3', 'the u64 fields read big endian', [('        u64::from_le_bytes(w)\n    };\n    let mut id', '        u64::from_be_bytes(w)\n    };\n    let mut id')]),
    ('SR4', 'the commit read from 33..65', [('        hex(&b[34..66]),', '        hex(&b[33..65]),')]),
    ('SR5', 'the snapshot id read little endian', [('        snap: ulid::Ulid::from_bytes(id),', '        snap: ulid::Ulid(u128::from_le_bytes(id)),')]),
    ('SR6', "the manifest's length read big endian", [('        manifest_len: u32::from_le_bytes(len),', '        manifest_len: u32::from_be_bytes(len),')]),
    ('SR7', 'the hash read from 85..117', [('    manifest_hash.copy_from_slice(&b[86..118]);', '    manifest_hash.copy_from_slice(&b[85..117]);')]),
    ('SR8', 'any version read at 118 bytes', [('    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {', '    if b.len() != SNAP_ROW_LEN {')]),
    ('SR9', 'a longer row read by its first 118 bytes', [('    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {', '    if b.len() < SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {')]),
    ('SR10', 'the version compared big endian', [('    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {', '    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_be_bytes() {')]),
    ('SR11', 'the version read by its low byte alone', [('    if b.len() != SNAP_ROW_LEN || b[..2] != 1u16.to_le_bytes() {', '    if b.len() != SNAP_ROW_LEN || b[0] != 1 {')]),
    ('SR12', "the manifest's hash not compared", [('        bytes.len() as u64 == u64::from(row.manifest_len) && *h.as_bytes() == row.manifest_hash;', '        bytes.len() as u64 == u64::from(row.manifest_len);')]),
    ('SR13', "the manifest's length not compared", [('        bytes.len() as u64 == u64::from(row.manifest_len) && *h.as_bytes() == row.manifest_hash;', '        *h.as_bytes() == row.manifest_hash;')]),
    ('SR14', 'the row read at the key snap/T (not skyzfs/snap/T)', [('    let key = format!("{SNAP_ROW_PREFIX}{}", a.snapshot);', '    let key = format!("snap/{}", a.snapshot);')]),
    ('SR15', 'the ref read as snaps/T', [('    let name = format!("snap/{}", a.snapshot);', '    let name = format!("snaps/{}", a.snapshot);')]),
    ('SR16', 'the manifest fetched from skyzfs/manifests/', [('        "{}skyzfs/manifest/{}",\n        skynbd_engine::keys::volume_prefix(vol),\n        r.snap\n    );\n    match store', '        "{}skyzfs/manifests/{}",\n        skynbd_engine::keys::volume_prefix(vol),\n        r.snap\n    );\n    match store')]),
    ('SR17', 'the prefix constant skyzfs/snaps/', [('const SNAP_ROW_PREFIX: &str = "skyzfs/snap/";', 'const SNAP_ROW_PREFIX: &str = "skyzfs/snaps/";')]),
    ('MH7', 'call site: an unguarded --row counts as guarded (the behaviour before N2)', [('    let guarded = expect.is_some();', '    let guarded = expect.is_some() || row.is_some();')]),
    ('MH7t', 'call site: guarded = true', [('    let guarded = expect.is_some();', '    let guarded = true;')]),
    ('MH7f', 'call site: guarded = false', [('    let guarded = expect.is_some();', '    let guarded = false;')]),
    ('MH7r', 'call site: guarded = row.is_some()', [('    let guarded = expect.is_some();', '    let guarded = row.is_some();')]),
    ('MHS', "the test store never moves head (its trigger misses the commit's key)", [('            let fire = key.contains("commit/")', '            let fire = key.contains("commits/")')]),
    ('MHA', 'the test store moves head after the PUT of the commit object (still before the swap)', [('            if fire {\n                let refs = Arc::new(ObjectRefStore::new(self.inner.clone()));', '            let put = self.inner.put(key, data.clone()).await;\n            if fire {\n                let refs = Arc::new(ObjectRefStore::new(self.inner.clone()));'), ('            self.inner.put(key, data).await\n        }\n        async fn get(', '            put\n        }\n        async fn get(')]),
]
DECODE = {p[0] for p in PLANTS if p[0].startswith('SR')}


def run_tests(wt, extra=''):
    path = os.path.join(wt, NB)
    orig = open(path).read()
    try:
        if extra:
            open(path, 'w').write(orig + extra)
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-nodeb', '--no-fail-fast'], cwd=wt, capture_output=True,
                           text=True, timeout=2400)
        rc, out = p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return 'timeout', [], ''
    finally:
        open(path, 'w').write(orig)
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
    rc2, failed2, note2 = run_tests(wt, XCHECK)
    print('baseline with the cross-check', rc2, failed2, note2, flush=True)
    if rc != 0 or rc2 != 0:
        sys.exit(2)
    results = []
    for pid, what, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, NB)
        orig = open(path).read()
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:50]!r})'
                break
            text = text.replace(old, new)
        res = {'id': pid, 'what': what, 'file': NB}
        if status is None:
            try:
                open(path, 'w').write(text)
                rc, failed, note = run_tests(wt)
                res.update(result='KILLED' if rc != 0 else 'SURVIVED', failed=failed, note=note)
                if pid in DECODE:
                    rc2, failed2, note2 = run_tests(wt, XCHECK)
                    res.update(with_xcheck='KILLED' if rc2 != 0 else 'SURVIVED', xfailed=failed2, xnote=note2)
            finally:
                open(path, 'w').write(orig)
        else:
            res.update(result=status)
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        res['clean_after'] = clean.strip() == ''
        results.append(res)
        print(pid, res.get('result'), res.get('failed'), res.get('note'),
              '| with the cross-check:', res.get('with_xcheck', '-'), res.get('xfailed', ''), flush=True)
        json.dump(results, open(outp, 'w'), indent=1)


if __name__ == '__main__':
    main()
