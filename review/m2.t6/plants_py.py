#!/usr/bin/env python3
"""Plants on the cut rig's judge, watcher and plan (#137, M2.T6).

Each plant replaces text that must occur exactly once in its file (else
NOT APPLIED), runs the python tests of tests/kernel that the change
touches, and restores the file in a finally. KILLED: a test failed;
SURVIVED: every test passed. Usage: plants_py.py WORKTREE OUT.json [ID...]
"""
import json
import os
import re
import subprocess
import sys

TESTS = ['tests/kernel/test_cut_daemon_main.py', 'tests/kernel/test_cut_daemon_check.py',
         'tests/kernel/test_cut_watch.py', 'tests/kernel/test_cut_plan.py']
CDC = 'tests/kernel/cut_daemon_check.py'
CW = 'tests/kernel/cut_watch.py'
CP = 'tests/kernel/cut_plan.py'

PLANTS = [
    ('PY1', 'uv1: a step that reads head_moved passes', CDC, [
        ("            if kind not in ('row_v1', 'matched'):",
         "            if kind not in ('row_v1', 'matched', 'head_moved'):")]),
    ('PY2', 'uv1: matched no longer accepted at a step', CDC, [
        ("            if kind not in ('row_v1', 'matched'):", "            if kind not in ('row_v1',):")]),
    ('PY3', 'uv1: upload_mismatches at least 1, not exactly 2', CDC, [
        ("        want.update(upload_mismatches=str(len(UV1_STEPS)),",
         "        want.update(upload_mismatches=at_least(1),")]),
    ('PY4', 'uv1: uploaded_whole at least 1, not exactly 3', CDC, [
        ("uploaded_whole=str(1 + len(UV1_STEPS))", "uploaded_whole=at_least(1)")]),
    ('PY5', "uv1: no check that others.tsv has the step's row of version 1", CDC, [
        ("            if first['cut'] not in v1:", "            if False:")]),
    ('PY6', "uv1: no check that the end row names head's commit", CDC, [
        ("        if end['commit'] is None or end['commit'] != end['head']:", "        if False:")]),
    ('PY7', 'uv1: no check that the course stays in one state', CDC, [
        ("            elif first['cut'][0] != state:", "            elif False:")]),
    ('PY8', 'uv1: no check of the row before each act (version 2 naming head)', CDC, [
        ("            if first['cut'] is None or first['commit'] != first['head']:",
         "            if first['cut'] is None:")]),
    ('PY9', 'uv1moved: row_v1 accepted once the mismatch was counted', CDC, [
        ("        if kind != 'head_moved':", "        if kind not in ('head_moved', 'row_v1'):")]),
    ('PY10', "uv1moved: no check that head is the other writer's at the end", CDC, [
        ("\"shows a move to another group\")\n        if end['head'] not in heads:",
         "\"shows a move to another group\")\n        if False:")]),
    ('PY11', 'uv1moved: no check of the row at the end', CDC, [
        ("        if (end['cut'], end['commit']) != (before['cut'], None):", "        if False:")]),
    ('PY12', 'uv1moved: the end status wants row_v1', CDC, [
        ("        n = stat('v1moved-status.txt').get('uploaded', '?')\n        want.update(upload_mismatches=at_least(1), "
         "upload_abandoned_at_mismatch=at_least(1),\n                    upload_checked='head_moved', uploaded=n)",
         "        n = stat('v1moved-status.txt').get('uploaded', '?')\n        want.update(upload_mismatches=at_least(1), "
         "upload_abandoned_at_mismatch=at_least(1),\n                    upload_checked='row_v1', uploaded=n)")]),
    ('PY13', 'row_commits_off_head finds nothing', CDC, [
        ("    heads, rows = others[:2]\n    return [f\"cut", "    heads, rows = others[:2]\n    return [] and [f\"cut")]),
    ('PY14', "row_commits_off_head does not leave out another writer's head or row", CDC, [
        ("            if g.get('commit') and g['commit'] != g['head']\n"
         "            and not (g['head'] in heads or (g['state'], g['number'], g['group']) in rows)]",
         "            if g.get('commit') and g['commit'] != g['head']]")]),
    ('PY15', "daemons_fetches skips a cut in v1 whatever the fetch's commit", CDC, [
        ("(g.get('commit') is None and cut in v1)", "(cut in v1)")]),
    ('PY16', 'daemons_fetches does not skip the row of version 1', CDC, [
        (" or (g.get('commit') is None and cut in v1):", ":")]),
    ('PY17', 'read_uploaded drops the commit', CDC, [
        ("'commit': f[7] if len(f) == 8 and f[7] != 'none' else None", "'commit': None")]),
    ('PY18', "read_row drops node B's row_commit", CDC, [
        ("            got['commit'] = None if f[1] == 'none' else f[1]", "            got['commit'] = None")]),
    ('PY19', 'STOPPED_AT without uv1moved', CDC, [
        (", 'uv1moved': 'v1moved-row.txt'}", "}")]),
    ('PY20', "uv1's second step reads the first step's status", CDC, [
        ("('v1in', 'v1in-row.txt', 'v1in-after.txt')", "('v1in', 'v1in-row.txt', 'v1-after.txt')")]),
    ('PY21', 'EXPECTED: mark-kept caught on uv1 (a single instance)', CDC, [
        ("    'mark-kept': dict({r: 'same' for r in RECS}, takeover='caught',",
         "    'mark-kept': dict({r: 'same' for r in RECS}, takeover='caught', uv1='caught',")]),
    ('PY22', 'EXPECTED: uv1 and uv1moved out of UNKNOWN (the three rules pinned same)', CDC, [
        ("UNKNOWN = {r: ('same', 'caught') for r in ('umoved', 'urow', 'ukill', 'ulease', 'ulost', 'uv1', 'uv1moved',",
         "UNKNOWN = {r: ('same', 'caught') for r in ('umoved', 'urow', 'ukill', 'ulease', 'ulost',")]),
    ('PY23', 'EXPECTED: mark-dropped caught on uv1moved', CDC, [
        ("    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught',",
         "    'mark-dropped': dict({r: 'same' for r in RECS}, takeover='caught', uv1moved='caught',")]),
    ('PY24', 'RECS without uv1 (no row of EXPECTED)', CDC, [
        ("        'umoved', 'urow', 'ukill', 'ulease', 'ulost', 'uv1', 'uv1moved',   # these where it uploads them",
         "        'umoved', 'urow', 'ukill', 'ulease', 'ulost', 'uv1moved',   # these where it uploads them")]),
    ('PY25', "the watcher's key leaves out the row's commit", CW, [
        ("        key = (before['head'], before['row'], before['commit'])", "        key = (before['head'], before['row'])"),
        ("        if after is None or (after['head'], after['row'], after['commit']) != key:",
         "        if after is None or (after['head'], after['row']) != key:")]),
    ('PY26', 'the watcher writes none for every commit', CW, [
        ("f\"{before['commit']}\\n\")", "f\"none\\n\")")]),
    ('PY27', "parse_row drops node B's row_commit", CW, [
        ("    commit = got.pop('row_commit', 'none')", "    got.pop('row_commit', None)\n    commit = 'none'")]),
    ('PY28', 'the plan runs uv1moved no more on a pull request', CP, [
        ("PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease ulost uv1 uv1moved'",
         "PR_UPLOAD_RECORDINGS = 'life umoved urow ukill ulease ulost uv1'")]),
]


def run_tests(wt):
    failed, outs = [], {}
    for t in TESTS:
        try:
            p = subprocess.run([sys.executable, '-I', t], cwd=wt, capture_output=True, text=True, timeout=900)
            rc, out = p.returncode, p.stdout + p.stderr
        except subprocess.TimeoutExpired:
            rc, out = 'timeout', ''
        names = re.findall(r'^(?:FAIL|ERROR): (\S+) \(([^)]*)\)', out, re.M)
        outs[t] = (rc, [f'{c.split(".")[-1]}.{n}' for n, c in names])
        if rc != 0:
            failed.append(t)
    return failed, outs


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    base_failed, _ = run_tests(wt)
    if base_failed:
        print('baseline fails:', base_failed)
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
        if status is None:
            try:
                open(path, 'w').write(text)
                failed, outs = run_tests(wt)
            finally:
                open(path, 'w').write(orig)
            status = 'KILLED' if failed else 'SURVIVED'
            detail = {t: outs[t] for t in failed}
        else:
            detail = {}
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        results.append({'id': pid, 'what': what, 'file': fn, 'result': status, 'failed': detail,
                         'clean_after': clean.strip() == ''})
        print(pid, status, json.dumps(detail)[:300], flush=True)
        json.dump(results, open(outp, 'w'), indent=1)


if __name__ == '__main__':
    main()
