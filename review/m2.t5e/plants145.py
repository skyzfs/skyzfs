#!/usr/bin/env python3
"""Review of #145 (M2.T5e) at cb91ad1: textual plants, one at a time,
each file put back in a finally, against the suites of the file planted
(each under a 600 s limit). KILLED: a suite failed (a time-out counts);
SURVIVED: every suite passed; NOT APPLIED: an old text is not in the file
exactly once.

- A: the agent's assist and its reading of the txgs kstat
  (publish_agent.py).
- K: the check's passist block and its helpers (publish_check.py).
- S: rec_passist, pidle and the course's wiring (cut-record.sh, the
  workflow).
- V1x, S14x, L4y, R46y, N24y: earlier reviews' plants (#143's V1 and
  S14; #141's L4, R46 and N24) aimed again at text this pull request
  changed.

The probe's lines (ublk-probe.sh) have no local suite: the kernel probe
is their only judge, so they are not planted here.

Usage: python3 -I plants145.py WORKTREE OUT.json [ID...]
"""
import json
import os
import subprocess
import sys

PA = 'tests/kernel/publish_agent.py'
PC = 'tests/kernel/publish_check.py'
SH = 'tests/kernel/cut-record.sh'
WF = '.github/workflows/skyzfs-publish-rig.yml'

T_PA = ['python3', '-I', 'tests/kernel/test_publish_agent.py']
T_PC = ['python3', '-I', 'tests/kernel/test_publish_check.py']
T_PCM = ['python3', '-I', 'tests/kernel/test_publish_check_main.py']
T_REC = ['bash', 'tests/kernel/test_cut_record.sh']
SUITES = {PA: [T_PA, T_PCM], PC: [T_PC, T_PCM], SH: [T_REC], WF: [T_REC]}

WT_LINE = "if len(f) >= 6 and f[0].isdigit() and f[2] == 'C' and f[5].isdigit() and int(f[5]) > 0:"
PREM = "bool(mine) and mine[0]['before'] < a1['group'] <= max(r['after'] for r in mine),"
PIN = "for k, r in enumerate(rows) if r['after'] <= r['before']]"
PIDLE = '$3 == "C" && $1 ~ /^[0-9]+$/ && $6 + 0 > 0 && $1 + 0 > t'
A1 = 'pflow a1 a1 --group-plus 5 --assist --limit-s 300'
A2 = 'pflow a2 a2 --group-plus 1000000 --expiry-s 900 --assist --limit-s 70'

PLANTS = [
    # The agent: written_txg.
    ('A1', PA, "written_txg without the state C", [(WT_LINE, WT_LINE.replace("f[2] == 'C' and ", ''))]),
    ('A2', PA, "written_txg without nwritten (any committed group)",
     [(WT_LINE, WT_LINE.replace(" and f[5].isdigit() and int(f[5]) > 0", ''))]),
    ('A3', PA, "written_txg with nwritten >= 0", [(WT_LINE, WT_LINE.replace('int(f[5]) > 0', 'int(f[5]) >= 0'))]),
    ('A4', PA, "written_txg skips the first group row too", [("    for ln in rows[1:]:\n", "    for ln in rows[2:]:\n")]),
    ('A5', PA, "written_txg: no group that wrote read as 0",
     [("raise RigError(f'{path}: no group that wrote in its history')", "return 0")]),
    # The agent: settled and assist.
    ('A6', PA, "settled returns the first read", [("    prev = synced(pool)\n    for _ in range(tries):",
                                                     "    prev = synced(pool)\n    return prev\n    for _ in range(tries):")]),
    ('A7', PA, "settled returns at the first change", [("        if cur == prev:\n", "        if cur != prev:\n")]),
    ('A8', PA, "the value without last + 1", [("value = max(wall() // 1_000_000, last + 1)", "value = wall() // 1_000_000")]),
    ('A9', PA, "the same value at every assist", [("value = max(wall() // 1_000_000, last + 1)", "value = 1")]),
    ('A10', PA, "no zpool sync after the set", [("    shell(['zpool', 'sync', pool])\n    t1 = mono()", "    t1 = mono()")]),
    ('A11', PA, "the sync before the set",
     [("    shell(['zfs', 'set', f'{ASSIST_PROP}={value}', pool])\n    mid = synced(pool)\n    shell(['zpool', 'sync', pool])\n",
       "    shell(['zpool', 'sync', pool])\n    mid = synced(pool)\n    shell(['zfs', 'set', f'{ASSIST_PROP}={value}', pool])\n")]),
    ('A12', PA, "after read as mid", [("    after = settled(pool, synced, sleep)\n", "    after = mid\n")]),
    ('A13', PA, "assist.tsv with mid and after swapped",
     [("{value}\\t{before}\\t{mid}\\t{after}\\n", "{value}\\t{before}\\t{after}\\t{mid}\\n")]),
    ('A14', PA, "another property", [("ASSIST_PROP = 'skyzfs:assist'", "ASSIST_PROP = 'skyzfs:nudge'")]),
    # The agent: the schedule.
    ('A15', PA, "a burst of one", [("ASSIST_BURST = 3", "ASSIST_BURST = 1")]),
    ('A16', PA, "a burst of four", [("ASSIST_BURST = 3", "ASSIST_BURST = 4")]),
    ('A17', PA, "after the burst, one a second (#144's engine at its head)",
     [("(assists < ASSIST_BURST or waited - assisted_at >= assist_after)",
       "(assists < ASSIST_BURST or waited - assisted_at >= 1)")]),
    ('A18', PA, "no wait before the first assist",
     [("if assist_after is not None and waited >= assist_after and \\", "if assist_after is not None and \\")]),
    ('A19', PA, "ASSIST_AFTER 5", [("ASSIST_AFTER = 10", "ASSIST_AFTER = 5")]),
    ('A20', PA, "main: --assist ignored", [("if '--assist' in opts", "if '--assist-x' in opts")]),
    ('A21', PA, "main: --assist-after-s ignored",
     [("float(val('--assist-after-s') or ASSIST_AFTER)", "float(ASSIST_AFTER)")]),
    # The check: the premise and the pin.
    ('K1', PC, "premise_a1 without 'below G at the first assist'",
     [(PREM, "bool(mine) and a1['group'] <= max(r['after'] for r in mine),")]),
    ('K2', PC, "premise_a1 without 'the assists reached G'", [(PREM, "bool(mine) and mine[0]['before'] < a1['group'],")]),
    ('K3', PC, "premise_a1 always holds", [(PREM, "True,")]),
    ('K4', PC, "the pin with < for <=", [(PIN, PIN.replace("r['after'] <= r['before']", "r['after'] < r['before']"))]),
    ('K5', PC, "the pin over a1's assists only", [(PIN, PIN.replace('enumerate(rows)', 'enumerate(mine)'))]),
    ('K6', PC, "the pin on the set's read (mid) for after",
     [(PIN, PIN.replace("r['after'] <= r['before']", "r['mid'] <= r['before']"))]),
    ('K7', PC, "the pin never fails", [(PIN, "for k, r in enumerate(rows) if False]")]),
    ('K8', PC, "premise_a2 with >= 0", [("len(a2) >= 2,", "len(a2) >= 0,")]),
    ('K9', PC, "a2's rows are every assist", [("a2 = [r for r in rows if r['name'] == 'a2']", "a2 = rows")]),
    ('K10', PC, "obs_a1_cut names no assist", [("by = [k + 1 for k, r in enumerate(mine) if r['before'] < row['group'] <= r['after']]",
                                              "by = []")]),
    # The check: kstat_groups, read_assists, assist_pace.
    ('K11', PC, "kstat_groups without the missing check",
     [("    if missing:\n        raise RigError(f'the txgs kstat lacks", "    if False:\n        raise RigError(f'the txgs kstat lacks")]),
    ('K12', PC, "kstat_groups with nwritten >= 0", [("seen[int(f[0])] = int(f[5]) > 0", "seen[int(f[0])] = int(f[5]) >= 0")]),
    ('K13', PC, "kstat_groups without the state C",
     [("if len(f) >= 6 and f[0].isdigit() and f[2] == 'C':\n            if not f[5].isdigit():",
       "if len(f) >= 6 and f[0].isdigit():\n            if not f[5].isdigit():")]),
    ('K14', PC, "kstat_groups over the first assist only",
     [("groups, empty = kstat_groups(txgs, rows[0]['before'], rows[-1]['after'])",
       "groups, empty = kstat_groups(txgs, rows[0]['before'], rows[0]['after'])")]),
    ('K15', PC, "read_assists takes a line with more fields", [("if len(f) != 7 or not f[0]", "if len(f) < 7 or not f[0]")]),
    ('K16', PC, "read_assists takes a non-number", [("not all(x.isdigit() for x in f[1:])", "False")]),
    ('K17', PC, "the pace's cuts not bounded by the assists",
     [("counted = len([c for c in cuts if rows[0]['t0'] <= c[0] <= rows[-1]['t1']])", "counted = len(cuts)")]),
    ('K18', PC, "the pace's uploads not a difference",
     [("uploaded = count(after, 'uploaded') - count(before, 'uploaded')", "uploaded = count(after, 'uploaded')")]),
    # The check: the tables and G3.
    ('K19', PC, "EXPECTED: a2-look snap", [("        'a2-look': ('absent',),\n", "        'a2-look': ('snap ',),\n")]),
    ('K20', PC, "EXPECTED: a1 pending", [("        'a1': ('done ',),\n        'a2': ('pending after',),",
                                         "        'a1': ('pending after',),\n        'a2': ('pending after',),")]),
    ('K21', PC, "PUBLISHED: passist none", [("    'passist': {'a1': 'a1'},\n", "    'passist': {},\n")]),
    ('K22', PC, "ABSENT: passist none", [(",\n          'passist': ('a2',)}", "}")]),
    ('K23', PC, "COURSES without passist", [(", 'pchannel', 'passist')", ", 'pchannel')")]),
    ('K24', PC, "G3 undone: an empty watch.log passes",
     [("        if not watch:\n            raise RigError('the watcher wrote no line to watch.log')\n", "")]),
    # #143's V1 (watch.log not read), aimed again at the text G3 changed.
    ('V1x', PC, "#143's V1 again: watch.log not read",
     [("        watch = text_of('watch.log').splitlines()\n"
       "        if not watch:\n"
       "            raise RigError('the watcher wrote no line to watch.log')\n"
       "        err = [ln.split('\\t', 1)[-1] for ln in watch if '\\tERROR ' in ln]\n"
       "        if err:\n"
       "            raise RigError(f'the watcher: {err[0]}')\n"
       "        bad = [ln.split('\\tVERDICT ', 1)[1] for ln in watch if '\\tVERDICT ' in ln]\n"
       "        holds('pin_watcher_reads', bad, 'kernel and server',\n"
       "              f'node B read the content of every cut the watcher found at head ({len(watch)} lines)')\n",
       "")]),
    # The recording.
    ('S1', SH, "pidle without nwritten", [(PIDLE, '$3 == "C" && $1 ~ /^[0-9]+$/ && $1 + 0 > t')]),
    ('S2', SH, "pidle idle after 1 s", [("[ $((SECONDS - since)) -lt 12 ] ||", "[ $((SECONDS - since)) -lt 1 ] ||")]),
    ('S3', SH, "pidle never waits", [("pidle() {\n    local t0", "pidle() {\n    return 0\n    local t0")]),
    ('S4', SH, "a1 at its snapshot's group", [(A1, A1.replace('--group-plus 5', '--group-plus 0'))]),
    ('S5', SH, "no pidle before a1", [("    load 1\n    pidle 120\n    pflow a1", "    load 1\n    pflow a1")]),
    ('S6', SH, "a1 without --assist", [(A1, A1.replace(' --assist', ''))]),
    ('S7', SH, "a2 without --assist", [(A2, A2.replace(' --assist', ''))]),
    ('S8', SH, "a2 with no expiry", [(A2, A2.replace(' --expiry-s 900', ''))]),
    ('S9', SH, "a2's txgs not kept", [('    cat "/proc/spl/kstat/zfs/$POOL/txgs" > "$RDIR/a2-txgs.txt"\n', '')]),
    ('S10', SH, "a2's statuses swapped",
     [("    ustatus a2-status-before.txt\n", "    ustatus a2-status-after.txt\n"),
      ("    ustatus a2-status-after.txt\n    cat", "    ustatus a2-status-before.txt\n    cat")]),
    ('S11', SH, "no withdrawal of a2", [("    pask a2-withdraw withdraw a2\n", "")]),
    ('S12', SH, "PUBLISH_NAMED without passist", [("pkillpub pchannel passist\"", "pkillpub pchannel\"")]),
    ('S13', SH, "zfs_txg_history not raised", [("echo 2000 > /sys/module/zfs/parameters/zfs_txg_history", ": 2000")]),
    ('S14', WF, "the workflow's default without passist",
     [('${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist}',
       '${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel}')]),
    # #141's L4, R46 and N24, as #143's review aimed them (reaim143.py),
    # aimed again at the tables and lists this pull request lengthened.
    ('L4y', PC, "#141's L4 again: f1 not in ABSENT", [("'prewindf': ('f1',), ", "")]),
    ('R46y', SH, "#141's R46 again: prewindf not a course by name",
     [('PUBLISH_NAMED="pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist"',
       'PUBLISH_NAMED="pplain prewind prewindc pkill pkillman pkillpub pchannel passist"')]),
    ('N24y', WF, "#141's N24 again: the workflow's default list without prewindf",
     [('${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist}',
       '${COURSES:-pplain prewind prewindc pkill pkillman pkillpub pchannel passist}')]),
    # #143's S14 (the default without prewindc), aimed again at the list
    # this pull request lengthened.
    ('S14x', WF, "#143's S14 again: the workflow's default without prewindc",
     [('${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist}',
       '${COURSES:-pplain prewind prewindf pkill pkillman pkillpub pchannel passist}')]),
]


def suites(wt, fn):
    bad = []
    for cmd in SUITES[fn]:
        try:
            p = subprocess.run(cmd, cwd=wt, capture_output=True, text=True, timeout=600)
            if p.returncode != 0:
                bad.append(os.path.basename(cmd[-1]))
        except subprocess.TimeoutExpired:
            bad.append(os.path.basename(cmd[-1]) + ' (TIMEOUT)')
    return bad


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    files = {fn for pid, fn, _, _ in PLANTS if not only or pid in only}
    base = {fn: suites(wt, fn) for fn in files}
    print('baseline', base, flush=True)
    if any(base.values()):
        sys.exit(2)
    results = []
    for pid, fn, what, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, fn)
        with open(path) as f:
            orig = f.read()
        mode = os.stat(path).st_mode
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:50]!r})'
                break
            text = text.replace(old, new)
        res = {'id': pid, 'file': fn, 'what': what}
        if status is None:
            try:
                with open(path, 'w') as f:
                    f.write(text)
                bad = suites(wt, fn)
            finally:
                with open(path, 'w') as f:
                    f.write(orig)
                os.chmod(path, mode)
            res.update(result='KILLED' if bad else 'SURVIVED', by=bad)
        else:
            res.update(result=status)
        res['clean_after'] = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True,
                                            text=True).stdout.strip() == ''
        results.append(res)
        print(f"{pid:4} {res['result']:9} {what}{'  by ' + ', '.join(res.get('by', [])) if res.get('by') else ''}",
              flush=True)
        with open(outp, 'w') as f:
            json.dump(results, f, indent=1)
    after = {fn: suites(wt, fn) for fn in files}
    print('restored', after, flush=True)


if __name__ == '__main__':
    main()
