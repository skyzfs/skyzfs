#!/usr/bin/env python3
"""#130 at 9710a24: the second look's plants on the new text (those of
the cap and of 'waited' dropped with what they planted in), and plants in
what 9710a24 changed: zdb_rig_code and its two users, the daemon jobs'
verdict, the busy-export record (S7) and its summary line. A plant in
replay_check.py or replay_daemon.py runs both rigs' judge tests.

    next_plants2.py CHECKOUT OUT [PREFIX ...]

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
D = 'tests/kernel/replay_daemon.py'
RT = [('py', 'tests/kernel/test_replay.py'), ('py', 'tests/kernel/test_replay_daemon.py')]
TESTS = {
    J: [('py', 'tests/kernel/test_cut_daemon_check.py'), ('py', 'tests/kernel/test_cut_daemon_main.py')],
    R: [('sh', 'tests/kernel/test_cut_record.sh')],
    P: [('py', 'tests/kernel/test_cut_plan.py')],
    X: RT,
    D: RT,
}
OKC = "    ok = int(held) >= 1 and sh.get('cut_holes') == 'held' and int(ab) == 0 and int(cp) < HELD_COPY_LIMIT\n"
GATE = "    if name in ('bound', 'boundsparse') and ver.isdigit() and int(ver) >= 17:\n"
EXIT2 = ("    if (left and not any(r['failed'] in JUDGING or r['failed'] == ZDB_ABORT for r in failures)\n"
         "            and all(r['match'] or rig_errors_explain(seen.get(r['name']), expected) for r in control_rows)\n"
         "            and witness_ok and (zdb_cc_rc == 0 or zdb_rig_code(zdb_cc_rc))):\n")
LEFT = ("    left = (any(LEFT_IMPORTED in r['failed'] for r in failures)\n"
        "            or any(LEFT_IMPORTED in e['failed'] for c in controls for e in c.get('rig_errors') or []))\n")
RB = ("        z 60 zpool sync \"$POOL\"\n    done\n    sleep 0.3\n    ustatus status-bound-held.txt\n"
      "    rm -f \"$WORK/hold.pause\"\n    load 2\n")
LON = "        rc, out = self.export()\n        if rc:\n            res = after_cleanup(res, 'export', out)\n        return res\n\n    def verify"
DSN = ("        rc, out = self.export()\n        if rc:\n            res = after_cleanup(res, 'export', out)\n        return res\n\n"
       "    def deep_imported")
LLD = "            res = after_cleanup(res, 'losetup -d', out)\n        return res[0], res[1], t\n"
DLD = "                res = after_cleanup(res, 'losetup -d', out)\n        return res[0], res[1], time.monotonic() - t0\n"
DMR = "            res = after_cleanup(res, 'dm-snapshot remove', out)\n"
EXPL = "    return not (want['by'] is None and c['caught'])\n"
NOC = "    if not c or not c.get('rig_errors'):\n        return False\n"

PLANTS = [
    # ---- the first look's plants on the new text: the judge
    ('J01 the limit is 8 MiB', J, 'HELD_COPY_LIMIT = 4 << 20\n', 'HELD_COPY_LIMIT = 8 << 20\n'),
    ('J02 the limit is inclusive', J, OKC, OKC.replace('int(cp) < HELD', 'int(cp) <= HELD')),
    ('J03 no cut need be held', J, OKC, OKC.replace('int(held) >= 1 and ', '')),
    ('J04 the holes need not be held', J, OKC, OKC.replace("sh.get('cut_holes') == 'held' and ", '')),
    ('J05 a cut may have been abandoned', J, OKC, OKC.replace(" and int(ab) == 0", '')),
    ('J06 the copies are not judged', J, OKC, OKC.replace(' and int(cp) < HELD_COPY_LIMIT', '')),
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
    ('J19 the gate is version 18', J, GATE, GATE.replace('>= 17', '>= 18')),
    ('J20 no gate on the version', J, GATE, "    if name in ('bound', 'boundsparse'):\n"),
    ('J21 the bound lines are never written', J, GATE, "    if False:\n"),
    ('J22b bound\'s abandoned line prints the copies read after the let-go again', J,
     "abandoned at the bound; the holder kept the '",
     "abandoned at the bound, with the copies '\n            f\"{status.get('cut_copied_bytes')!r} bytes of "
     "{status.get('cut_bound_bytes')!r}; the holder kept the \""),
    # ---- S3: new
    ('J23 (S3) a held reading without cut_holes is judged', J, " or sh.get('cut_holes') is None:", ":"),
    ('J24 (S3) a missing cuts_abandoned counts as none abandoned', J,
     "cp.isdigit() and ab.isdigit())", "cp.isdigit())", "and int(ab) == 0 and", "and ab in ('0', '') and"),
    # ---- the recorder
    ('R01 the held reading after the let-go', R, RB,
     "        z 60 zpool sync \"$POOL\"\n    done\n    sleep 0.3\n    rm -f \"$WORK/hold.pause\"\n"
     "    ustatus status-bound-held.txt\n    load 2\n"),
    ('R02 the held reading before the last sync', R, RB,
     "        ustatus status-bound-held.txt\n        z 60 zpool sync \"$POOL\"\n    done\n    sleep 0.3\n"
     "    rm -f \"$WORK/hold.pause\"\n    load 2\n"),
    ('R03 no held reading', R, RB, RB.replace("    ustatus status-bound-held.txt\n", '')),
    ('R04 the held reading under another name', R, RB, RB.replace('status-bound-held.txt', 'status-held.txt')),
    ('R05 the held reading after the loads', R, RB + '    export_pool\n',
     RB.replace("    ustatus status-bound-held.txt\n", '') + "    ustatus status-bound-held.txt\n    export_pool\n"),
    ('R06 the pause after the create', R, "    touch \"$WORK/hold.pause\"\n    create_pool\n    for k in",
     "    create_pool\n    touch \"$WORK/hold.pause\"\n    for k in"),
    ('R07 nine writes', R, '    for k in $(seq 1 10); do\n        dd if=/dev/urandom of="$MNT/b$k"',
     '    for k in $(seq 1 9); do\n        dd if=/dev/urandom of="$MNT/b$k"'),
    ('R08 (S2) no wait before the held reading', R, RB, RB.replace("    sleep 0.3\n", '')),
    ('R09 (S2) the wait after the held reading', R, RB,
     RB.replace("    sleep 0.3\n    ustatus status-bound-held.txt\n", "    ustatus status-bound-held.txt\n    sleep 0.3\n")),
    ('R10 (S2) a wait shorter than the status interval and a write', R, RB, RB.replace("sleep 0.3", "sleep 0.1")),
    # ---- the plan
    ('P01 a docs/ path in UPLOAD_PATHS', P, "UPLOAD_PATHS = (\n", "UPLOAD_PATHS = (\n    'docs/',\n"),
    ('P02 a tests/kernel/ path that the workflow does not name', P, "UPLOAD_PATHS = (\n",
     "UPLOAD_PATHS = (\n    'tests/kernel/replay_daemon.py',\n"),
    # ---- the busy export (first look's, on the new text)
    ('X01 no retry', X, 'EXPORT_WAITS = (1, 2, 4, 8)\n', 'EXPORT_WAITS = ()\n'),
    ('X02 any failure is retried', X, "            if 'busy' not in out:\n                break\n",
     "            if False:\n                break\n"),
    ('X03 nothing recorded', X, "            self.held(out, wait)\n", ''),
    ('X04 no -f after the waits', X,
     "        rc, out, _ = run(['zpool', 'export', '-f', self.a.pool], 120)\n        if busy:", "        if busy:"),
    ('X05 no waits', X, "            time.sleep(wait)\n", ''),
    ('X06 fuser not asked on a mounted file system', X, "            _, fu, _ = run(['fuser', '-vm', mp], 30)\n",
     "            fu = ''\n"),
    ('X07b the record has prefix 0 for every busy export (in held)', X, "b = {'export': self.exports, 'at': self.at,",
     "b = {'export': self.exports, 'at': 0,"),
    ('X07c the forced record has prefix 0', X, "self.forced.append({'export': self.exports, 'at': self.at,",
     "self.forced.append({'export': self.exports, 'at': 0,"),
    ('X08 the mounts are not recorded', X, "and w[1].startswith(self.alt)]", "and False]"),
    ('X09 the record does not say what the kernel said', X, "'said': said.strip()[-200:],", "'said': '',"),
    ('X10 one wait fewer', X, 'EXPORT_WAITS = (1, 2, 4, 8)\n', 'EXPORT_WAITS = (1, 2, 4)\n'),
    # ---- exit 2 (first look's, on the new text)
    ('X11 a judging failure does not keep exit 1', X, EXIT2,
     EXIT2.replace("not any(r['failed'] in JUDGING or r['failed'] == ZDB_ABORT for r in failures)\n            and ", '')),
    ('X12 a control judged otherwise does not keep exit 1', X, EXIT2,
     EXIT2.replace("            and all(r['match'] or rig_errors_explain(seen.get(r['name']), expected) for r in control_rows)\n", '')),
    ('X13 the witness does not keep exit 1', X, EXIT2, EXIT2.replace("witness_ok and ", '')),
    ('X14 zdb does not keep exit 1', X, EXIT2, EXIT2.replace(" and (zdb_cc_rc == 0 or zdb_rig_code(zdb_cc_rc))", '')),
    ('X15 a leftover seen only in a control is missed', X, LEFT,
     "    left = any(LEFT_IMPORTED in r['failed'] for r in failures)\n"),
    ('X16 a leftover seen only in the main pass is missed', X, LEFT,
     "    left = any(LEFT_IMPORTED in e['failed'] for c in controls for e in c.get('rig_errors') or [])\n"),
    ('X17 never 2', X, "        return 2\n    return 1\n", "        return 1\n    return 1\n"),
    ('X18 2 whenever a pool was left', X, EXIT2, "    if left:\n"),
    ('X19 leftover() says it otherwise', X, "            return f'rig: a pool named {self.a.pool} {LEFT_IMPORTED}'\n",
     "            return f'rig: a pool named {self.a.pool} remains imported'\n"),
    ('X20 main exits 0 or 1', X, "    sys.exit(code)\n\n\ndef held_text", "    sys.exit(0 if ok else 1)\n\n\ndef held_text"),
    ('X21 the summary has no rig lines', X, "    out.extend(rig_lines(res))\n", ''),
    ('X22 the summary does not say exit 2', X, "    if res.get('exit_code') == 2:\n", "    if False:\n"),
    ('X23 export_busy not in the results', X, "    res['export_busy'] = rig.busy\n", ''),
    # ---- F1: after_cleanup
    ('A01 (F1) a cleanup failure always replaces the result', X,
     "    if res[0]:\n        return res[0], f'{res[1]}; then {what} failed: {detail}'\n",
     "    if False:\n        return res[0], f'{res[1]}; then {what} failed: {detail}'\n"),
    ('A02 (F1) light_on: the export replaces', X, LON,
     LON.replace("res = after_cleanup(res, 'export', out)", "res = ('export', out.strip()[-400:])")),
    ('A03 (F1) deep_snapshot: the export replaces', X, DSN,
     DSN.replace("res = after_cleanup(res, 'export', out)", "res = ('export', out.strip()[-400:])")),
    ('A04 (F1) light: losetup -d replaces', X, LLD,
     LLD.replace("res = after_cleanup(res, 'losetup -d', out)", "res = ('losetup -d', out.strip()[-400:])")),
    ('A05 (F1) deep: losetup -d replaces', X, DLD,
     DLD.replace("res = after_cleanup(res, 'losetup -d', out)", "res = ('losetup -d', out.strip()[-400:])")),
    ('A06 (F1) deep_loops: dm-snapshot remove replaces', X, DMR,
     "            res = ('dm-snapshot remove', out.strip()[-400:])\n"),
    ('A07 (F1) the cleanup\'s failure left out of the detail', X,
     "        return res[0], f'{res[1]}; then {what} failed: {detail}'\n", "        return res[0], res[1]\n"),
    ('A08 (F1) a passing check whose cleanup failed passes', X, "    return what, detail\n", "    return None, ''\n"),
    # ---- F2: rig_errors_explain
    ('B01 (F2) a catch the table does not expect is explained', X, EXPL, "    return True\n"),
    ('B02 (F2) a class the table does not name is explained', X,
     "    if want is None:\n        return False\n    return not", "    if want is None:\n        return True\n    return not"),
    ('B03 (F2) the old rule: any rig error excuses', X,
     "all(r['match'] or rig_errors_explain(seen.get(r['name']), expected) for r in control_rows)",
     "all(r['match'] or bool((seen.get(r['name']) or {}).get('rig_errors')) for r in control_rows)"),
    ('B04 (F2) a control without rig errors is explained', X, NOC, "    if not c:\n        return False\n"),
    ('B05 (F2) a control that did not run is explained', X, NOC,
     "    if not c:\n        return True\n    if not c.get('rig_errors'):\n        return False\n"),
    ('B06 (F2) the class defaults otherwise', X,
     "    want = expected.get(c['name'], {}).get(c.get('class', 'block'))\n",
     "    want = expected.get(c['name'], {}).get(c.get('class') or 'superseded')\n"),
    ('B07 (F2) exit_code ignores the table it is given', X,
     "    expected = EXPECTED if expected is None else expected\n    if not c or",
     "    expected = EXPECTED\n    if not c or"),
    # ---- the lead's rule and S5
    ('C01 (lead) zdb\'s abort in the main pass is not counted', X, EXIT2, EXIT2.replace(" or r['failed'] == ZDB_ABORT", '')),
    ('C02 (lead) any zdb signal in the main pass counts', X, EXIT2,
     EXIT2.replace("r['failed'] == ZDB_ABORT", "r['failed'].startswith('rig: zdb died with signal')")),
    ('C03 (lead) zdb\'s abort in a control is not a catch', X, "        if r['failed'] == ZDB_ABORT:\n",
     "        if False:\n"),
    ('C04 (S5) zdb\'s rig codes at the end are not excused', X, EXIT2,
     EXIT2.replace("(zdb_cc_rc == 0 or zdb_rig_code(zdb_cc_rc))", "zdb_cc_rc == 0")),
    ('C05 (S5) every zdb exit code at the end is excused', X, EXIT2,
     EXIT2.replace("(zdb_cc_rc == 0 or zdb_rig_code(zdb_cc_rc))", "True")),
    ('C06 (S5, lead) zdb\'s abort at the end is excused too', X, EXIT2,
     EXIT2.replace("(zdb_cc_rc == 0 or zdb_rig_code(zdb_cc_rc))", "(zdb_cc_rc <= 0 or zdb_rig_code(zdb_cc_rc))")),
    # ---- F3: Rig.held
    ('E01 (F3) fuser asked on an unmounted directory', X, "        if mp in mounts:\n            _, fu, _", "        if True:\n            _, fu, _"),
    ('E02 (F3) fuser\'s header kept', X, " or w[:2] == ['USER', 'PID']", ""),
    ('E03 (F3) the kernel mount line kept', X, " or w[1] == 'kernel'", ""),
    ('E04 (F3) the path on the first line not dropped', X, "        if w and w[0].endswith(':'):\n            w = w[1:]\n", ""),
    ('E05 (F3) ps matched by arguments', X, "if len(w) >= 3 and w[2] in ZFS_COMMANDS:",
     "if len(w) >= 4 and any(z in w[3] for z in ZFS_COMMANDS):"),
    ('E06 (F3) ps not asked', X, "        _, ps, _ = run(['ps', '-eo', 'pid=,etimes=,comm=,args='], 30)\n", "        ps = ''\n"),
    ('E07 (F3) zed not in the set', X, "ZFS_COMMANDS = ('zpool', 'zfs', 'zdb', 'zed')", "ZFS_COMMANDS = ('zpool', 'zfs', 'zdb')"),
    ('E08 (F3) zpool not in the set', X, "ZFS_COMMANDS = ('zpool', 'zfs', 'zdb', 'zed')", "ZFS_COMMANDS = ('zfs', 'zdb', 'zed')"),
    ('E09 (F3) fuser\'s tail kept, not its head', X, "'fuser': fu.strip()[:1000]", "'fuser': fu.strip()[-1000:]"),
    ('E10 (F3) mounted never recorded', X, "'mounted': mp in mounts,", "'mounted': False,"),
    ('E11 (F3) the summary says unmounted whatever was seen', X, "    if b.get('mounted'):\n", "    if False:\n"),
    ('E12 (F3) the ZFS commands left out of the words', X,
     "    return fs + '; ZFS commands running: ' + ('; '.join(b.get('zfs_processes', [])[:4]) or 'none')\n",
     "    return fs\n"),
    # ---- F4 and S4
    ('G01 (F4) refusals counted as exports', X, "        exports = len({x['export'] for x in busy})\n",
     "        exports = len(busy)\n"),
    ('G02 (F4) the export number not counted', X, "        self.exports += 1\n", ""),
    ('G03 (F4) forced exports not recorded', X, "            self.forced.append({", "            dict({"),
    ('G04 (F4) export_forced not in the results', X, "    res['export_forced'] = rig.forced\n", ""),
    ('G06 (F4) the -f exit code not recorded', X, "'at': self.at, 'rc': rc})", "'at': self.at, 'rc': 0})"),
    # ---- N1: the cap is gone; every busy export waits
    ('H05 (N1) the cap planted again: after two forced, no waits', X, "        for wait in EXPORT_WAITS + (None,):\n",
     "        for wait in (EXPORT_WAITS if len(self.forced) < 2 else ()) + (None,):\n"),
    ('H06 (N1) only the first export waits', X, "        for wait in EXPORT_WAITS + (None,):\n",
     "        for wait in (EXPORT_WAITS if self.exports == 1 else ()) + (None,):\n"),
    ('H07 (N1) a refusal on the last try not recorded', X,
     "            self.held(out, wait)\n            if wait is None:\n                break\n",
     "            if wait is None:\n                break\n            self.held(out, wait)\n"),
    # ---- N3: the summary line
    ('G07 (N3) every forced export counted as exported', X, "        gone = sum(1 for f in forced if f['rc'] == 0)\n",
     "        gone = len(forced)\n"),
    ('G08 (N3) the failed -f counted as exported', X, "        gone = sum(1 for f in forced if f['rc'] == 0)\n",
     "        gone = sum(1 for f in forced if f['rc'] != 0)\n"),
    ('G09 (N3) the line does not say how many were left', X,
     "f\"and left {len(forced) - gone} imported (-f exit codes \"", "f\"(-f exit codes \""),
    ('G10 (N3) the line says "exported with -f" again', X,
     "still busy after every wait and sent to -f, which exported {gone} '", "still busy after every wait and exported with -f '"),
    # ---- N4 (the lead): zdb_rig_code
    ('K01 (N4) SIGABRT counted as the rig\'s', X, "(rc < 0 and rc != -6)", "rc < 0"),
    ('K02 (N4) no signal counted as the rig\'s', X, "rc in RIG_RC or (rc < 0 and rc != -6)", "rc in RIG_RC"),
    ('K03 (N4) a timeout or a missing zdb counted against the image', X,
     "rc in RIG_RC or (rc < 0 and rc != -6)", "(rc < 0 and rc != -6)"),
    ('K04 (N4) None counted as the rig\'s', X, "    return rc is not None and (", "    return rc is None or ("),
    ('K05 (N4) exit_code back to RIG_RC at the end', X, EXIT2,
     EXIT2.replace("zdb_rig_code(zdb_cc_rc)", "zdb_cc_rc in RIG_RC")),
    ('K06 (N4) the daemon judge counts a signal against the daemon again', D,
     "    if zdb_cc_rc not in (0, None) and not rc.zdb_rig_code(zdb_cc_rc):\n",
     "    if zdb_cc_rc not in (0, None) and zdb_cc_rc not in rc.RIG_RC:\n"),
    ('K07 (N4) the daemon judge\'s rig line back to RIG_RC', D, "    elif rc.zdb_rig_code(zdb_cc_rc):\n",
     "    elif zdb_cc_rc in rc.RIG_RC:\n"),
    ('K08 (N4) the daemon judge\'s line does not say the signal', D, "else f'killed by signal {-zdb_cc_rc}')",
     "else 'did not finish')"),
    ('K09 (N4) the daemon judge counts any signal both ways', D,
     "    if zdb_cc_rc not in (0, None) and not rc.zdb_rig_code(zdb_cc_rc):\n",
     "    if zdb_cc_rc not in (0, None) and zdb_cc_rc not in rc.RIG_RC:\n",
     "    elif rc.zdb_rig_code(zdb_cc_rc):\n", "    if rc.zdb_rig_code(zdb_cc_rc):\n"),
    ('K10 (N4) the daemon judge\'s main pass: zdb\'s abort is the rig\'s', D,
     "    if r['failed'] in JUDGING or r['failed'] == 'rig: zdb died with signal 6':\n",
     "    if r['failed'] in JUDGING:\n"),
    ('K10b (N4) the daemon judge\'s main pass: zdb\'s abort is the rig\'s (classify)', D,
     "(rig_errors).\"\"\"\n    if r['failed'] in JUDGING or r['failed'] == 'rig: zdb died with signal 6':\n",
     "(rig_errors).\"\"\"\n    if r['failed'] in JUDGING:\n"),
    ('K10c (N4) the daemon\'s controls: zdb\'s abort is not a catch (tally)', D,
     "            continue\n        if r['failed'] in JUDGING or r['failed'] == 'rig: zdb died with signal 6':\n",
     "            continue\n        if r['failed'] in JUDGING:\n"),
    ('K11 (N4) the main pass: a signal is a finding of zdb', X,
     "        return f'rig: zdb died with signal {-rc}', ' | '.join(said)[:400], counts\n",
     "        return 'copies: zdb', ' | '.join(said)[:400], counts\n"),
    ('K12 (N4, M1) the stopped path judged by zdb_rig_code (the fix; should survive)', D,
     "    zdb_against = zcc not in (0, None) and zcc not in rc.RIG_RC\n",
     "    zdb_against = zcc not in (0, None) and not rc.zdb_rig_code(zcc)\n"),
    ('K13 (N4, M1) the stopped path: any nonzero zdb against the daemon', D,
     "    zdb_against = zcc not in (0, None) and zcc not in rc.RIG_RC\n",
     "    zdb_against = zcc not in (0, None)\n"),
    ('K14 (N4, M1) the stopped path: zdb never against the daemon', D,
     "    zdb_against = zcc not in (0, None) and zcc not in rc.RIG_RC\n",
     "    zdb_against = False\n"),
    # ---- S7: the daemon jobs' record
    ('L01 (S7) the prefix not set', D,
     "        self.rig.at = n                 # the prefix a busy export's record names (Rig.held)\n", ""),
    ('L02 (S7) the prefix set after the image is made', D,
     "        self.rig.at = n                 # the prefix a busy export's record names (Rig.held)\n", "",
     "        failed, detail, info = self.daemon.make(n, self.rig.image, hooks)\n",
     "        failed, detail, info = self.daemon.make(n, self.rig.image, hooks)\n        self.rig.at = n\n"),
    ('L03 (S7) export_busy not in the daemon\'s results', D,
     "    res['export_busy'], res['export_forced'] = ck.rig.busy, ck.rig.forced\n", ""),
    ('L04 (S7) export_forced not in the daemon\'s results', D,
     "    res['export_busy'], res['export_forced'] = ck.rig.busy, ck.rig.forced\n",
     "    res['export_busy'], res['export_forced'] = ck.rig.busy, []\n"),
    ('L05 (S7) the daemon\'s summary has no busy-export line', D,
     "    out.extend(rc.rig_lines({k: res.get(k) for k in ('export_busy', 'export_forced')}))\n", ""),
    ('L06 (S7) the daemon\'s summary passes the whole results (an exit 2 line from a daemon exit)', D,
     "    out.extend(rc.rig_lines({k: res.get(k) for k in ('export_busy', 'export_forced')}))\n",
     "    out.extend(rc.rig_lines(dict(res, exit_code=res.get('exit'))))\n"),
    ('L07 (S7, M2) the deep check names its own prefix (the fix; should survive)', D,
     "        self.use(p['dir'])\n", "        self.rig.at = p['n']\n        self.use(p['dir'])\n"),
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
