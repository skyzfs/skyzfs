#!/usr/bin/env python3
"""Third look at #145 (M2.T5e) at 58fc660: textual plants on what 58fc660
changed, one at a time, each file put back in a finally, against the
suites of the file planted (each under a 600 s limit). KILLED: a suite
failed (a time-out counts); SURVIVED: every suite passed; NOT APPLIED:
an old text is not in the file exactly once. For each plant the suites'
seconds are recorded (the pidle plants that loop must end through the
test's own `timeout 20`, not through this harness's 600 s).

- F: pace_faults and the passist block (publish_check.py): the floors
  (S1 as the lead decided), the burst's pause from the end of the assist
  before (S2), the scaling for the fixtures' ns clock, the slow
  observations (obs_assist_gap_over_3x) and the exit code of a pace
  fault.
- C: the export refusals' count (cut_check.py, S3).
- P: pidle (cut-record.sh), against test_cut_record.sh: the plants that
  B2's fix must still kill, among them a pidle that never sleeps, which
  with SECONDS unset only the test's limit ends.

The fix of B2 itself (`unset SECONDS`) cannot be planted away this way:
without it the test is wrong only when a second of the wall clock turns
during a run. pidle_head.sh and pidle_load.sh count that.

Usage: python3 -I third_plants.py WORKTREE OUT.json [ID...]
"""
import json
import os
import subprocess
import sys
import time

CC = 'tests/kernel/cut_check.py'
PC = 'tests/kernel/publish_check.py'
SH = 'tests/kernel/cut-record.sh'

T_CC = ['python3', '-I', 'tests/kernel/test_cut_check.py']
T_PC = ['python3', '-I', 'tests/kernel/test_publish_check.py']
T_PCM = ['python3', '-I', 'tests/kernel/test_publish_check_main.py']
T_REC = ['bash', 'tests/kernel/test_cut_record.sh']
SUITES = {CC: [T_CC], PC: [T_PC, T_PCM], SH: [T_REC]}

FIRST = '        if first < a:\n            bad.append('
BURST_GAP = "            gap = rows[k]['t0'] - rows[k - 1]['t1']\n"
SPACED_GAP = "        gap = rows[k]['t0'] - rows[k - 1]['t0']\n"
PAUSE = '    pause = a / publish_agent.ASSIST_AFTER\n'
CALL = "                    premise(f'premise_{t}_assists_at_the_decided_pace', not early,\n"
COUNT = '                self.export_refusals += len(refused)\n'
PUT = "        put('obs_export_refusals', 'INFO', f'{ck.export_refusals} busy exports"

PLANTS = [
    # S1 (the lead's decision): the floors are the premise's verdict.
    ('F1', PC, "no first-assist floor", [(FIRST, '        if False:\n            bad.append(')]),
    ('F2', PC, "the first-assist floor at A / 2", [(FIRST, '        if first < a / 2:\n            bad.append(')]),
    ('F3', PC, "S2 undone: the burst's pause from the start of the assist before",
     [(BURST_GAP, "            gap = rows[k]['t0'] - rows[k - 1]['t0']\n")]),
    ('F4', PC, "no floor in the burst", [('            if gap < pause:\n', '            if False:\n')]),
    ('F5', PC, "the burst's floor not scaled: 1 s on every clock", [(PAUSE, '    pause = 1e9\n')]),
    ('F6', PC, "the burst's floor half the agent's pause", [(PAUSE, '    pause = a / publish_agent.ASSIST_AFTER / 2\n')]),
    ('F7', PC, "the burst's floor A / 2 (the old ceiling's value)", [(PAUSE, '    pause = a / 2\n')]),
    ('F8', PC, "the spaced floor 0.9 A", [('        if gap < 0.95 * a:\n', '        if gap < 0.9 * a:\n')]),
    ('F9', PC, "the spaced floor A exactly (no tolerance for the kstat read before the set)",
     [('        if gap < 0.95 * a:\n', '        if gap < a:\n')]),
    ('F10', PC, "the spaced floor 1.04 A", [('        if gap < 0.95 * a:\n', '        if gap < 1.04 * a:\n')]),
    ('F11', PC, "the spaced gap from the end of the assist before", [(SPACED_GAP, SPACED_GAP.replace("['t0']\n", "['t1']\n"))]),
    ('F12', PC, "a burst of two", [('        if k < publish_agent.ASSIST_BURST:\n', '        if k < 2:\n')]),
    ('F13', PC, "a burst of four", [('        if k < publish_agent.ASSIST_BURST:\n', '        if k < 4:\n')]),
    ('F14', PC, "the ceilings made verdicts", [(CALL, CALL.replace('not early,', 'not (early or late),'))]),
    ('F15', PC, "a slow first over 2 A", [('        if first > 3 * a:\n', '        if first > 2 * a:\n')]),
    ('F16', PC, "a slow pending end from the start of the last assist",
     [("        last = rows[-1]['t1'] if rows else accepted_ns\n", "        last = rows[-1]['t0'] if rows else accepted_ns\n")]),
    ('F17', PC, "a slow end of any verb", [("    if end is not None and end[2] == 'pending':\n", '    if end is not None:\n')]),
    ('F18', PC, "the end mark not passed", [("marks[t]['accepted'][2], marks[t].get('end'))", "marks[t]['accepted'][2], None)")]),
    ('F19', PC, "a1's slow gaps lost (a2's replace them)", [('                    slow += late\n', '                    slow = late\n')]),
    ('F20', PC, "no line obs_assist_gap_over_3x", [("            put('obs_assist_gap_over_3x', 'INFO', 'rig', ", "            (lambda *x: None)('obs_assist_gap_over_3x', 'INFO', 'rig', ")]),
    ('F21', PC, "the decided ASSIST_AFTER for the mark's A", [("marks[t]['accepted'][2], marks[t].get('end'))", "publish_agent.ASSIST_AFTER, marks[t].get('end'))")]),
    ('F22', PC, "a pace fault the daemon's (exit 1), the line ok", [(CALL, '                    rc = max(rc, 1 if early else 0)\n                    early = []\n' + CALL)]),
    ('F23', PC, "a slow burst over 2 pauses", [('            if gap > 3 * pause:\n', '            if gap > 2 * pause:\n')]),
    ('F24', PC, "no slow spaced gap", [('        if gap > 3 * a:\n            slow.append', '        if False:\n            slow.append')]),
    # S3: the export refusals' count.
    ('C1', CC, "the refusals not counted", [(COUNT, '                pass\n')]),
    ('C2', CC, "the refusals counted only when the export failed", [(COUNT, '                self.export_refusals += len(refused) if erc else 0\n')]),
    ('C3', CC, "the count reset at each import", [(COUNT, '                self.export_refusals = len(refused)\n')]),
    ('C4', CC, "no line obs_export_refusals", [(PUT, PUT.replace("        put('obs_export_refusals'", "        (lambda *x: None)('obs_export_refusals'"))]),
    ('C5', CC, "the line says 0", [(PUT, PUT.replace('{ck.export_refusals}', '{0}'))]),
    # B2: what the fixed test must still kill.
    ('P1', SH, "pidle never sleeps (it loops reading the kstat)", [('        sleep 1\n    done\n}\n# LABEL T SECONDS [sync]', '        :\n    done\n}\n# LABEL T SECONDS [sync]')]),
    ('P2', SH, "pidle sleeps 0 s", [('        sleep 1\n    done\n}\n# LABEL T SECONDS [sync]', '        sleep 0\n    done\n}\n# LABEL T SECONDS [sync]')]),
    ('P3', SH, "pidle idle after 1 s (S2 of the first look)", [('[ $((SECONDS - since)) -lt 12 ] ||', '[ $((SECONDS - since)) -lt 1 ] ||')]),
    ('P4', SH, "pidle idle after 11 s", [('[ $((SECONDS - since)) -lt 12 ] ||', '[ $((SECONDS - since)) -lt 11 ] ||')]),
    ('P5', SH, "pidle idle after 13 s", [('[ $((SECONDS - since)) -lt 12 ] ||', '[ $((SECONDS - since)) -le 12 ] ||')]),
    ('P6', SH, "pidle's limit one second late", [('[ $((SECONDS - t0)) -lt "$1" ] ||', '[ $((SECONDS - t0)) -le "$1" ] ||')]),
    ('P7', SH, "pidle never waits (S3 of the first look)", [('pidle() {\n    local t0', 'pidle() {\n    return 0\n    local t0')]),
    ('P8', SH, "pidle's limit never an error", [('[ $((SECONDS - t0)) -lt "$1" ] ||', '[ 1 ] ||')]),
    ('P9', SH, "pidle sleeps 2 s", [('        sleep 1\n    done\n}\n# LABEL T SECONDS [sync]', '        sleep 2\n    done\n}\n# LABEL T SECONDS [sync]')]),
]


def suites(wt, fn):
    bad = []
    t = time.monotonic()
    for cmd in SUITES[fn]:
        try:
            p = subprocess.run(cmd, cwd=wt, capture_output=True, text=True, timeout=600)
            if p.returncode != 0:
                tail = [ln for ln in (p.stdout + p.stderr).splitlines() if 'FAIL' in ln or 'Error' in ln][:2]
                bad.append(os.path.basename(cmd[-1]) + (f" ({' | '.join(tail)[:300]})" if tail else ''))
        except subprocess.TimeoutExpired:
            bad.append(os.path.basename(cmd[-1]) + ' (TIMEOUT)')
    return bad, round(time.monotonic() - t, 1)


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    files = {fn for pid, fn, _, _ in PLANTS if not only or pid in only}
    base = {fn: suites(wt, fn) for fn in files}
    print('baseline', base, flush=True)
    if any(b for b, _ in base.values()):
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
                bad, secs = suites(wt, fn)
            finally:
                with open(path, 'w') as f:
                    f.write(orig)
                os.chmod(path, mode)
            res.update(result='KILLED' if bad else 'SURVIVED', by=bad, seconds=secs)
        else:
            res.update(result=status)
        res['clean_after'] = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True,
                                            text=True).stdout.strip() == ''
        results.append(res)
        print(f"{pid:4} {res['result']:9} {what}{' (' + str(res['seconds']) + ' s)' if 'seconds' in res else ''}"
              f"{'  by ' + '; '.join(res.get('by', [])) if res.get('by') else ''}", flush=True)
        with open(outp, 'w') as f:
            json.dump(results, f, indent=1)
    after = {fn: suites(wt, fn)[0] for fn in files}
    print('restored', after, flush=True)


if __name__ == '__main__':
    main()
