#!/usr/bin/env python3
"""Plants on node B's row codec and move-head --row, and on the
uploader's row codec (#137, M2.T6: G3 of the review of #134).

Each plant replaces text that must occur exactly once in its file (else
NOT APPLIED), runs `cargo test -p skyzfs-nodeb --no-fail-fast` (2400 s
limit), and restores the file in a finally. KILLED: a test failed or the
build failed; SURVIVED: every test passed. Usage:
plants_rs.py WORKTREE OUT.json [ID...]
"""
import json
import os
import re
import subprocess
import sys

NB = 'bins/skyzfs-nodeb/src/main.rs'
ROW = 'crates/skyzfs-upload/src/row.rs'

PLANTS = [
    # node B's codec
    ('NB1', 'node B reads a row of version 1 at any length from 34', NB, [
        ("        (ROW_LEN_V1, Some([1, 0])) => false,", "        (l, Some([1, 0])) if l >= ROW_LEN_V1 => false,")]),
    ('NB2', "node B reads the number at the group's offset and the group at the number's", NB, [
        ("    n.copy_from_slice(&b[18..26]);\n    g.copy_from_slice(&b[26..34]);",
         "    n.copy_from_slice(&b[26..34]);\n    g.copy_from_slice(&b[18..26]);")]),
    ('NB3', 'node B reads the commit from bytes 2..34', NB, [
        ("        c.copy_from_slice(&b[34..66]);", "        c.copy_from_slice(&b[2..34]);")]),
    ('NB4', 'node B writes the version big-endian', NB, [
        ("    let mut b = version.to_le_bytes().to_vec();", "    let mut b = version.to_be_bytes().to_vec();")]),
    ('NB5', "node B reads version 2 by its low byte alone", NB, [
        ("        (ROW_LEN_V2, Some([2, 0])) => true,", "        (ROW_LEN_V2, Some([2, _])) => true,")]),
    ('NB6', 'node B swaps the number and the group of a row of version 1 only', NB, [
        ("    g.copy_from_slice(&b[26..34]);\n    let commit = v2.then",
         "    g.copy_from_slice(&b[26..34]);\n    if !v2 {\n        std::mem::swap(&mut n, &mut g);\n    }\n"
         "    let commit = v2.then")]),
    ('NB7', 'node B labels every row version 2', NB, [
        ("    let version: u16 = if c.commit.is_some() { 2 } else { 1 };", "    let version: u16 = 2;")]),
    ('NB8', "node B's version 2 is 67 bytes", NB, [
        ("const ROW_LEN_V2: usize = 66;", "const ROW_LEN_V2: usize = 67;")]),
    ('NB9', 'node B also reads version 3 as version 2', NB, [
        ("        (ROW_LEN_V2, Some([2, 0])) => true,", "        (ROW_LEN_V2, Some([2 | 3, 0])) => true,")]),
    # the uploader's codec
    ('RW1', 'the uploader reads a row longer than its version says', ROW, [
        ("    if bytes.len() != len {", "    if bytes.len() < len {")]),
    ('RW2', 'the uploader writes the group before the number', ROW, [
        ("    row.extend_from_slice(&cut.number.to_le_bytes());\n    row.extend_from_slice(&cut.group.to_le_bytes());",
         "    row.extend_from_slice(&cut.group.to_le_bytes());\n    row.extend_from_slice(&cut.number.to_le_bytes());")]),
    ('RW3', "the uploader's row version is 3", ROW, [
        ("pub const ROW_VERSION: u16 = 2;", "pub const ROW_VERSION: u16 = 3;")]),
    ('RW4', 'the uploader reads the commit from bytes 33..65', ROW, [
        ("        _ => match bytes.get(34..66)", "        _ => match bytes.get(33..65)")]),
    ('RW5', 'the uploader reads the version big-endian', ROW, [
        ("        [a, b, ..] => u16::from_le_bytes([*a, *b]),", "        [a, b, ..] => u16::from_be_bytes([*a, *b]),")]),
    ('RW6', 'the uploader gives a row of version 1 a commit of zeros', ROW, [
        ("        1 => None,", "        1 => Some([0u8; 32]),")]),
    ('RW7', 'the uploader reads any version from 2 as version 2', ROW, [
        ("        ROW_VERSION => ROW_LEN,", "        v if v >= ROW_VERSION => ROW_LEN,")]),
    # move-head --row
    ('MH1', 'move-head under a guard keeps the row it read and drops --row', NB, [
        ("            let value = row\n                .or_else(|| expected.clone())",
         "            let value = expected\n                .clone()\n                .or(row)")]),
    ('MH2', 'move-head writes the row in a second transaction after the commit (not atomic)', NB, [
        ("    let ops = row_op(expect, row)?;\n    let mut eng = Engine::open(store, refs, vol, EngineConfig::default())",
         "    let later = row.clone();\n    let refs2 = Arc::clone(&refs);\n"
         "    let ops = if expect.is_some() { row_op(expect, None).ok().flatten() } else { None };\n"
         "    let mut eng = Engine::open(store, refs, vol, EngineConfig::default())"),
        ("        Ok(head) => Ok(Some(head)),\n        // The row, or head with it",
         "        Ok(head) => {\n            if let Some(value) = later {\n"
         "                let _ = refs2.transact(vol, vec![RefOp::MetaPut { key: ROW_KEY.to_string(), value }]).await;\n"
         "            }\n            Ok(Some(head))\n        }\n        // The row, or head with it")]),
    ('MH3', 'move-head refuses --row v1:', NB, [
        ("            spec @ (RowSpec::V1(_) | RowSpec::Bytes(_) | RowSpec::WithCommit(_)) => {",
         "            spec @ (RowSpec::Bytes(_) | RowSpec::WithCommit(_)) => {")]),
    ('MH4', 'move-head takes --row STATE:NUMBER:GROUP', NB, [
        ("            spec @ (RowSpec::V1(_) | RowSpec::Bytes(_) | RowSpec::WithCommit(_)) => {",
         "            spec @ (RowSpec::V1(_) | RowSpec::Bytes(_) | RowSpec::WithCommit(_) | RowSpec::Cut(..)) => {")]),
    ('MH5', 'move-head with --row and no guard writes no row', NB, [
        ("        (None, Some(value)) => Some(RefOp::MetaPut { key, value }),", "        (None, Some(_)) => None,")]),
    ('MH6', 'move-head with --row writes it whatever the row is now (guard dropped)', NB, [
        ("            Some(RefOp::MetaCas {\n                key,\n                expected,\n                value,\n            })\n        }\n"
         "        (None, Some(value))",
         "            let _ = expected;\n            Some(RefOp::MetaPut { key, value })\n        }\n"
         "        (None, Some(value))")]),
]

PLANTS2 = [
    ('NB4b', 'node B writes the version big-endian', NB, [
        ("    let version: u16 = if c.commit.is_some() { 2 } else { 1 };\n    let mut b = version.to_le_bytes().to_vec();",
         "    let version: u16 = if c.commit.is_some() { 2 } else { 1 };\n    let mut b = version.to_be_bytes().to_vec();")]),
    ('MH2b', 'move-head commits head under the guard alone, then puts the row in a second transaction (not atomic)', NB, [
        ("    let ops = row_op(expect, row)?;\n    let mut eng = Engine::open(store, refs, vol, EngineConfig::default())",
         "    let (ops, later) = match (&expect, &row) {\n"
         "        (Some(Some(b)), Some(r)) => (\n"
         "            Some(RefOp::MetaCas { key: ROW_KEY.to_string(), expected: Some(b.clone()), value: b.clone() }),\n"
         "            Some(r.clone()),\n"
         "        ),\n"
         "        (None, Some(r)) => (None, Some(r.clone())),\n"
         "        _ => (row_op(expect, row)?, None),\n"
         "    };\n"
         "    let refs2 = Arc::clone(&refs);\n"
         "    let mut eng = Engine::open(store, refs, vol, EngineConfig::default())"),
        ("        Ok(head) => Ok(Some(head)),\n        // The row, or head with it",
         "        Ok(head) => {\n            if let Some(value) = later {\n"
         "                let _ = refs2.transact(vol, vec![RefOp::MetaPut { key: ROW_KEY.to_string(), value }]).await;\n"
         "            }\n            Ok(Some(head))\n        }\n        // The row, or head with it")]),
    ('MH7', 'move-head with --row and no guard: a lost head swap is an error, not exit 6 (as before --row)', NB, [
        ("    let guarded = ops.is_some();", "    let guarded = matches!(ops, Some(RefOp::MetaCas { .. }));")]),
]
PLANTS = PLANTS2 if os.environ.get('PLANTS2') else PLANTS


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
    for pid, what, fn, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, fn)
        orig = open(path).read()
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:50]!r})'
                break
            text = text.replace(old, new)
        failed, note = [], ''
        if status is None:
            try:
                open(path, 'w').write(text)
                rc, failed, note = run_tests(wt)
            finally:
                open(path, 'w').write(orig)
            status = 'KILLED' if rc != 0 else 'SURVIVED'
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        results.append({'id': pid, 'what': what, 'file': fn, 'result': status, 'failed': failed, 'note': note,
                        'clean_after': clean.strip() == ''})
        print(pid, status, failed, note, flush=True)
        json.dump(results, open(outp, 'w'), indent=1)


if __name__ == '__main__':
    main()
