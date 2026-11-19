#!/usr/bin/env python3
"""#130 at c205682: plants, one at a time, in what the pull request
changed, against the tests of the changed files.

    plants.py CHECKOUT OUT [PREFIX ...]

A plant replaces a text of a file by another. One whose text is not
found exactly once says NOT APPLIED. The file is restored after each
(try/finally), each result is appended to OUT as it comes, a plant whose
name OUT already holds is skipped, and the script refuses to start on a
checkout that `git status --short` does not call clean. Each test file
runs under a limit of its own; one that does not end in it counts as
failed. KILLED means a test failed, which is what a plant should do.
"""
import os
import subprocess
import sys

ROOT, OUT = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
J = 'tests/kernel/cut_daemon_check.py'
R = 'tests/kernel/cut-record.sh'
P = 'tests/kernel/cut_plan.py'
X = 'tests/kernel/replay_check.py'
TESTS = {
    J: [('py', 'tests/kernel/test_cut_daemon_check.py'), ('py', 'tests/kernel/test_cut_daemon_main.py')],
    R: [('sh', 'tests/kernel/test_cut_record.sh')],
    P: [('py', 'tests/kernel/test_cut_plan.py')],
    X: [('py', 'tests/kernel/test_replay.py')],
}
OKC = ("    ok = (int(held) >= 1 and sh.get('cut_holes') == 'held' and sh.get('cuts_abandoned') == '0'\n"
       "          and int(cp) < HELD_COPY_LIMIT)\n")
GATE = "    if name in ('bound', 'boundsparse') and ver.isdigit() and int(ver) >= 17:\n"
EXIT2 = ("    if (left and not any(r['failed'] in JUDGING for r in failures)\n"
         "            and all(r['match'] or r['name'] in errored for r in control_rows)\n"
         "            and witness_ok and zdb_cc_rc == 0):\n")
LEFT = ("    left = (any(LEFT_IMPORTED in r['failed'] for r in failures)\n"
        "            or any(LEFT_IMPORTED in e['failed'] for c in controls for e in c.get('rig_errors') or []))\n")
RB = ("        z 60 zpool sync \"$POOL\"\n    done\n    ustatus status-bound-held.txt\n    rm -f \"$WORK/hold.pause\"\n"
      "    load 2\n")

PLANTS = [
    # ---- the judge: cut_copied_held
    ('J01 the limit is 8 MiB', J, 'HELD_COPY_LIMIT = 4 << 20\n', 'HELD_COPY_LIMIT = 8 << 20\n'),
    ('J02 the limit is inclusive', J, 'and int(cp) < HELD_COPY_LIMIT)', 'and int(cp) <= HELD_COPY_LIMIT)'),
    ('J03 no cut need be held', J, OKC, OKC.replace('int(held) >= 1 and ', '')),
    ('J04 the holes need not be held', J, OKC, OKC.replace("sh.get('cut_holes') == 'held' and ", '')),
    ('J05 a cut may have been abandoned', J, OKC, OKC.replace(" and sh.get('cuts_abandoned') == '0'", '')),
    ('J06 the copies are not judged', J, OKC, OKC.replace('\n          and int(cp) < HELD_COPY_LIMIT)', ')')),
    ('J07 the copies judged on status-held.txt (read after the stop, before the drain)', J,
     "    sh = read_status(read_file(rdir, 'status-bound-held.txt'))\n",
     "    sh = read_status(read_file(rdir, 'status-held.txt'))\n"),
    ('J08 a missing held reading is an ERROR line, not the rig\'s error', J,
     "    sh = read_status(read_file(rdir, 'status-bound-held.txt'))\n",
     "    sh = read_status(read_file(rdir, 'status-bound-held.txt') if os.path.exists(os.path.join(rdir, "
     "'status-bound-held.txt')) else '')\n"),
    ('J09 a FAIL of the copies does not set the exit', J, "    return (rc if ok else 1), errored\n",
     "    return rc, errored\n"),
    ('J10 a held reading that cannot be judged is not an error', J,
     "the status read while the first cut was held cannot be judged')\n        return rc, True\n",
     "the status read while the first cut was held cannot be judged')\n        return rc, errored\n"),
    ('J11 bound\'s copies are judged as boundsparse\'s', J,
     "    if name == 'bound':\n        put('cut_copied_held', 'INFO',", "    if False:\n        put('cut_copied_held', 'INFO',"),
    # ---- the judge: bound_premise
    ('J12 bound\'s premise asks nothing', J, "        ok = int(chunks) == 0\n", "        ok = True\n"),
    ('J13 boundsparse\'s premise does not ask for held holes', J,
     "        ok = sh_holes == 'held' and int(chunks) > 0\n", "        ok = int(chunks) > 0\n"),
    ('J14 boundsparse\'s premise does not ask for chunks', J,
     "        ok = sh_holes == 'held' and int(chunks) > 0\n", "        ok = sh_holes == 'held'\n"),
    ('J15 the premise is read from the held reading', J,
     "    st = read_status(read_file(rdir, 'status-start-1.txt'))\n",
     "    st = read_status(read_file(rdir, 'status-bound-held.txt'))\n"),
    ('J16 a premise that cannot be read is not an error', J,
     "the rig cannot tell what the volume held where the state began')\n        errored = True\n",
     "the rig cannot tell what the volume held where the state began')\n"),
    ('J17 a premise FAIL does not set the exit (bound)', J,
     "'(--zero-fill), so no chunk of it may be a hole')\n        rc = rc if ok else 1\n",
     "'(--zero-fill), so no chunk of it may be a hole')\n"),
    ('J18 a premise FAIL does not set the exit (boundsparse)', J,
     "'with the whole volume, so the daemon holds its holes')\n        rc = rc if ok else 1\n",
     "'with the whole volume, so the daemon holds its holes')\n"),
    # ---- the judge: the gate
    ('J19 the gate is version 18', J, GATE, GATE.replace('>= 17', '>= 18')),
    ('J20 no gate on the version', J, GATE, "    if name in ('bound', 'boundsparse'):\n"),
    ('J21 the bound lines are never written', J, GATE, "    if False:\n"),
    ('J22 bound\'s abandoned line prints the copies read after the let-go again', J,
     "abandoned at the bound; the holder kept the '",
     "abandoned at the bound, with the copies '\n            f\"{status.get('cut_copied_bytes')!r} bytes of "
     "{status.get('cut_bound_bytes')!r}; the holder kept the '"),   # a syntax error: not a plant
    ('J22b bound\'s abandoned line prints the copies read after the let-go again', J,
     "abandoned at the bound; the holder kept the '",
     "abandoned at the bound, with the copies '\n            f\"{status.get('cut_copied_bytes')!r} bytes of "
     "{status.get('cut_bound_bytes')!r}; the holder kept the \""),
    # ---- the recorder: rec_bound's order
    ('R01 the held reading after the let-go', R, RB,
     "        z 60 zpool sync \"$POOL\"\n    done\n    rm -f \"$WORK/hold.pause\"\n    ustatus status-bound-held.txt\n"
     "    load 2\n"),
    ('R02 the held reading before the last sync', R, RB,
     "        ustatus status-bound-held.txt\n        z 60 zpool sync \"$POOL\"\n    done\n    rm -f \"$WORK/hold.pause\"\n"
     "    load 2\n"),
    ('R03 no held reading', R, RB, "        z 60 zpool sync \"$POOL\"\n    done\n    rm -f \"$WORK/hold.pause\"\n    load 2\n"),
    ('R04 the held reading under another name', R, RB, RB.replace('status-bound-held.txt', 'status-held.txt')),
    ('R05 the held reading after the loads', R, RB + '    export_pool\n',
     "        z 60 zpool sync \"$POOL\"\n    done\n    rm -f \"$WORK/hold.pause\"\n    load 2\n"
     "    ustatus status-bound-held.txt\n    export_pool\n"),
    ('R06 (pre-existing) the pause after the create', R, "    touch \"$WORK/hold.pause\"\n    create_pool\n    for k in",
     "    create_pool\n    touch \"$WORK/hold.pause\"\n    for k in"),
    ('R07 nine writes', R, '    for k in $(seq 1 10); do\n        dd if=/dev/urandom of="$MNT/b$k"',
     '    for k in $(seq 1 9); do\n        dd if=/dev/urandom of="$MNT/b$k"'),
    # ---- the plan
    ('P01 a docs/ path in UPLOAD_PATHS', P, "UPLOAD_PATHS = (\n", "UPLOAD_PATHS = (\n    'docs/',\n"),
    ('P02 a tests/kernel/ path that the workflow does not name', P, "UPLOAD_PATHS = (\n",
     "UPLOAD_PATHS = (\n    'tests/kernel/replay_daemon.py',\n"),
    # ---- the replay rig: the busy export
    ('X01 no retry', X, 'EXPORT_WAITS = (1, 2, 4, 8)\n', 'EXPORT_WAITS = ()\n'),
    ('X02 any failure is retried', X, "            if 'busy' not in out or wait is None:\n",
     "            if wait is None:\n"),
    ('X03 nothing recorded', X, "            self.held(out, wait)\n", ''),
    ('X04 no -f after the waits', X,
     "        rc, out, _ = run(['zpool', 'export', '-f', self.a.pool], 120)\n        return rc, out\n\n    def held",
     "        return rc, out\n\n    def held"),
    ('X05 no waits', X, "            time.sleep(wait)\n", ''),
    ('X06 fuser not asked', X, "        _, fu, _ = run(['fuser', '-vm', self.alt + self.a.mountpoint], 30)\n",
     "        fu = ''\n"),
    ('X07 the record has prefix 0 for every busy export', X, "b = {'at': self.at,", "b = {'at': 0,"),
    ('X08 the mounts are not recorded', X, "and w[1].startswith(self.alt)]", "and False]"),
    ('X09 the record does not say what the kernel said', X, "'said': said.strip()[-200:],", "'said': '',"),
    ('X10 one wait fewer', X, 'EXPORT_WAITS = (1, 2, 4, 8)\n', 'EXPORT_WAITS = (1, 2, 4)\n'),
    # ---- the replay rig: exit 2
    ('X11 a judging failure does not keep exit 1', X, EXIT2,
     EXIT2.replace("not any(r['failed'] in JUDGING for r in failures)\n            and ", '')),
    ('X12 a control judged otherwise does not keep exit 1', X, EXIT2,
     EXIT2.replace("            and all(r['match'] or r['name'] in errored for r in control_rows)\n", '')),
    ('X13 the witness does not keep exit 1', X, EXIT2, EXIT2.replace("witness_ok and ", '')),
    ('X14 zdb does not keep exit 1', X, EXIT2, EXIT2.replace(" and zdb_cc_rc == 0", '')),
    ('X15 a leftover seen only in a control is missed', X, LEFT,
     "    left = any(LEFT_IMPORTED in r['failed'] for r in failures)\n"),
    ('X16 a leftover seen only in the main pass is missed', X, LEFT,
     "    left = any(LEFT_IMPORTED in e['failed'] for c in controls for e in c.get('rig_errors') or [])\n"),
    ('X17 never 2', X, "        return 2\n    return 1\n", "        return 1\n    return 1\n"),
    ('X18 2 whenever a pool was left', X, EXIT2, "    if left:\n"),
    ('X19 leftover() says it otherwise', X, "            return f'rig: a pool named {self.a.pool} {LEFT_IMPORTED}'\n",
     "            return f'rig: a pool named {self.a.pool} remains imported'\n"),
    ('X20 main exits 0 or 1', X, "    sys.exit(code)\n\n\ndef rig_lines", "    sys.exit(0 if ok else 1)\n\n\ndef rig_lines"),
    ('X21 the summary has no rig lines', X, "    out.extend(rig_lines(res))\n", ''),
    ('X22 the summary does not say exit 2', X, "    if res.get('exit_code') == 2:\n", "    if False:\n"),
    ('X23 export_busy not in the results', X, "    res['export_busy'] = rig.busy\n", ''),
    ('X24 a control whose mismatch the rig errors do not explain is excused (as now; Codex): planted the fix', X,
     "    errored = {c['name'] for c in controls if c.get('rig_errors')}\n",
     "    errored = {c['name'] for c in controls if c.get('rig_errors') and not (c.get('class') == 'superseded' "
     "and c.get('caught'))}\n"),
]


def run_tests(env, path):
    """-> the names of what failed, [] when every test passes."""
    for kind, t in TESTS[path]:
        cmd, limit = ([sys.executable, '-B', t], 300) if kind == 'py' else (['bash', t], 300)
        try:
            p = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=limit)
        except subprocess.TimeoutExpired:
            return [f'{os.path.basename(t)} did not end in {limit} s']
        if kind == 'py':
            failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL: ', 'ERROR: '))})
        else:
            failed = [f'{os.path.basename(t)}: {ln[:90]}' for ln in p.stdout.splitlines()
                      if ln.startswith(('FAIL ', 'not ok', 'FAIL:'))]
        if p.returncode != 0 and not failed:
            failed = [f'{os.path.basename(t)} exit {p.returncode}: {(p.stderr or p.stdout).strip()[-120:]}']
        if failed:
            return failed
    return []


def main():
    dirty = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True, timeout=120)
    if dirty.returncode != 0 or dirty.stdout.strip():
        sys.exit(f'{ROOT} is not a clean checkout: {dirty.stdout.strip() or dirty.stderr.strip()}')
    done = []
    if os.path.exists(OUT):
        with open(OUT) as f:
            done = f.read().splitlines()
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    with open(OUT, 'a') as out:
        def say(text):
            print(text, flush=True)
            out.write(text + '\n')
            out.flush()
        for name, path, *pairs in PLANTS:
            if ONLY and not name.startswith(ONLY):
                continue
            if any(ln.startswith(name + ': ') for ln in done):
                continue
            full = os.path.join(ROOT, path)
            with open(full) as f:
                orig = f.read()
            counts = [orig.count(old) for old in pairs[0::2]]
            if any(c != 1 for c in counts) or any(old == new for old, new in zip(pairs[0::2], pairs[1::2])):
                say(f'{name}: NOT APPLIED (found {counts} times)')
                continue
            text = orig
            for old, new in zip(pairs[0::2], pairs[1::2]):
                text = text.replace(old, new)
            try:
                with open(full, 'w') as f:
                    f.write(text)
                failed = run_tests(env, path)
            finally:
                with open(full, 'w') as f:
                    f.write(orig)
            say(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:4])}' if failed else 'SURVIVED'))
    dirty = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True, timeout=120)
    print('worktree after: ' + ('clean' if not dirty.stdout.strip() else dirty.stdout.strip()))


if __name__ == '__main__':
    main()
