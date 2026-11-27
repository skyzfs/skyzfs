#!/usr/bin/env python3
"""Plants in tests/kernel/agent_fail_check.py (and its hand-off in
agent_check.py), one at a time, each against the no-kernel tests:
test_agent_fail_check.py (which also runs agent_check.py's hand-off).

Usage: plants.py TREE OUT

TREE is a clean worktree of the head; each plant is a textual
replacement that must match exactly once; the file is restored with
`git checkout` after each. OUT gets one line per plant: id, result
(killed / SURVIVES / not-applied / timeout), and the first failing test.
"""
import os
import re
import subprocess
import sys

F = 'tests/kernel/agent_fail_check.py'
A = 'tests/kernel/agent_check.py'

PLANTS = [
    # Tolerances.
    ('T1', 'STEP_WITHIN 3 s -> 30 s', F, 'STEP_WITHIN = 3.0', 'STEP_WITHIN = 30.0'),
    ('T2', 'END_WITHIN 2 s -> 60 s', F, 'END_WITHIN = 2.0', 'END_WITHIN = 60.0'),
    ('T3', 'TICK_SLACK 1 s -> 30 s', F, 'TICK_SLACK = 1.0', 'TICK_SLACK = 30.0'),
    ('T4', 'LOOK_LIMIT_MS 10.5 s -> 60 s', F, 'LOOK_LIMIT_MS = 10_500', 'LOOK_LIMIT_MS = 60_000'),
    ('T5', 'EVERY_DEFAULT 60 -> 30', F, 'EVERY_DEFAULT = 60', 'EVERY_DEFAULT = 30'),
    ('T6', 'the schedule one-sided (abs dropped)', F, 'if abs(got - want) > TICK_SLACK:', 'if got - want > TICK_SLACK:'),
    ('T7', 'the schedule without late_ms', F,
     "want = (r['f']['n'] - 1) * EVERY_DEFAULT + (r['f']['late_ms'] - r1['f']['late_ms']) / 1000",
     "want = (r['f']['n'] - 1) * EVERY_DEFAULT"),
    ('T8', 'step_within measured from the failure, not the note', F,
     "[] if at - note <= STEP_WITHIN else", "[] if at - c['fail'][0] <= STEP_WITHIN + 1 else"),
    # U's lost line.
    ('U1', 'U held unknown or yes', F, "'held': 'unknown',\n", "'held': ('unknown', 'yes'),\n"),
    ('U2', 'U verdict failed or lost', F, "'verdict': 'failed'})", "'verdict': ('failed', 'lost')})"),
    ('U3', 'U step not-imported or reopened', F, "'step': 'not-imported', 'reopen': '-'",
     "'step': ('not-imported', 'reopened'), 'reopen': '-'"),
    ('U4', 'U: no round 3 not checked', F, "        bad.append('no round 3: the run did not go on')\n", "        pass\n"),
    ('U5', 'U: round 2 failed not checked', F, "            bad.append('round 2 failed')\n", "            pass\n"),
    ('U6', 'U: round 2 begun not checked', F, "            bad.append(f\"round 2 began {r2['f']['begun']}\")\n",
     "            pass\n"),
    ('U7', "U: round 2's words not checked", F,
     "        bad.append('no line on standard error that round 2 found the pool not imported and goes on')\n",
     "        pass\n"),
    ('U8', 'U: the line after lost need not be round 2', F,
     "if nxt is None or nxt['kind'] != 'round' or nxt['f']['n'] != 2:",
     "if nxt is None or nxt['kind'] != 'round':"),
    ('U9', 'U: one lost line not counted', F, "    if len(lost) != 1:\n        bad.append(f'{len(lost)} lost lines, not 1')\n    if k is None:",
     "    if not lost:\n        bad.append(f'{len(lost)} lost lines, not 1')\n    if k is None:"),
    ('U10', 'U refused: rc 4 not checked', F, "    if c['refused_rc'] != '4':", "    if c['refused_rc'] not in ('4', '0'):"),
    ('U11', 'U refused: the note after it not checked', F,
     "        bad.append('no failure note after the refused start')\n", "        pass\n"),
    ('U12', 'U unheld: a lost line in round 3 allowed', F, "            bad.append('a lost line in round 3')\n",
     "            pass\n"),
    ('U13', 'U unheld: round 3 failed allowed', F, "            bad.append('round 3 failed')\n", "            pass\n"),
    ('U14', "U unheld: round 3's words not checked", F,
     "            bad.append('no line on standard error that round 3 found no client held the device')\n",
     "            pass\n"),
    ('U15', 'U unheld: round 3 begun allowed', F, "            bad.append(f\"round 3 began {r3['f']['begun']}\")\n",
     "            pass\n"),
    ('U16', 'U moved: 12 or 10', F, "    if c['rc'] != '12':", "    if c['rc'] not in ('12', '10'):"),
    ('U17', 'U moved: the last line not checked', F, "    elif lines[-1] is not r4:", "    elif False:"),
    ('U18', "U moved: round 4's words not checked", F,
     "            bad.append('no line on standard error that round 4 saw the device or its attachment change')\n",
     "            pass\n"),
    ('U19', 'U schedule: rounds missing pass', F,
     "        bad.append('rounds 1, 3 and 4 are not all there to time')\n", "        pass\n"),
    ('U20', 'U lost line: the pool not checked', F,
     "bad += lost_wrong(f, {'pool': POOL, 'note': 'whole'", "bad += lost_wrong(f, {'note': 'whole'"),
    # The daemon's end.
    ('D1', 'daemon end 3 not checked', F, "    if failed_rc != '3':", "    if failed_rc not in ('3', '137'):"),
    ('D2', 'the note before the failure allowed', F, '    elif mtime < fail_real - 0.05:', '    elif mtime < fail_real - 60:'),
    # W and the second run.
    ('W1', 'W: 11 or 9', F, "    if rc != '11':", "    if rc not in ('11', '9'):"),
    ('W2', 'W: verdict lost or retry', F, "'state': 'SUSPENDED', 'verdict': 'lost', 'step': step})",
     "'state': 'SUSPENDED', 'verdict': ('lost', 'retry'), 'step': step})"),
    ('W3', 'W: state SUSPENDED or ONLINE', F, "'state': 'SUSPENDED', 'verdict': 'lost', 'step': step})",
     "'state': ('SUSPENDED', 'ONLINE'), 'verdict': 'lost', 'step': step})"),
    ('W4', 'W: held yes or unknown', F, "'reason': REASON, 'held': 'yes',\n",
     "'reason': REASON, 'held': ('yes', 'unknown'),\n"),
    ('W5', 'W: a suspended step with a reopen allowed', F,
     "        bad += lost_wrong(lo['f'], {'reopen': '-', 'looked_ms': '-'})\n", "        pass\n"),
    ('W6', 'W: looked_ms not checked', F,
     "        if lo['f']['looked_ms'] == '-' or int(lo['f']['looked_ms']) > LOOK_LIMIT_MS:",
     "        if lo['f']['looked_ms'] == '-':"),
    ('W7', 'W: the lost round began allowed', F, "        bad.append(f\"the lost round began {rd['f']['begun']}\")\n",
     "        pass\n"),
    ('W8', 'W: the end time not checked', F,
     "    if rd['at'] is None or ended_at is None or ended_at - rd['at'] > END_WITHIN:", "    if False:"),
    ('W9', 'W: the last two lines not checked', F,
     "    if len(lines) < 2 or lines[-2]['kind'] != 'lost' or lines[-1]['kind'] != 'round':",
     "    if len(lines) < 2 or not any(x['kind'] == 'lost' for x in lines):"),
    ('W10', 'W: round 1 before the lost line not checked', F,
     "    if r1 is None or lo is None or index(run, r1) > index(run, lo):", "    if lo is None:"),
    ('W11', 'W: the kstat state not checked', F, "[] if st == 'SUSPENDED' else", "[] if st in ('SUSPENDED', 'ONLINE') else"),
    ('W12', 'again: step reopened allowed', F, "a['run'], a['rc'], a['ended_at'], 'suspended')",
     "a['run'], a['rc'], a['ended_at'], ('reopened', 'suspended'))"),
    ('W13', 'again: the rounds not checked', F,
     "    if [r['f']['n'] for r in rounds(a['run'])] != [1]:", "    if not rounds(a['run']):"),
    ('W14', 'W: reopen ok/failed not checked', F, "        bad += lost_wrong(lo['f'], {'reopen': ('ok', 'failed')})\n",
     "        pass\n"),
    ('W15', 'W: the step not checked', F, "    bad, end, lo = judge_lost_end(run, c['rc'], c['ended_at'], ('reopened', 'suspended'))",
     "    bad, end, lo = judge_lost_end(run, c['rc'], c['ended_at'], ('reopened', 'suspended', 'not-imported', 'failed'))"),
    # Premises.
    ('P1', 'W premise: asked not needed', F, "    bad = [] if asked else ['the trace did not reach asked:after before the failure']",
     "    bad = []"),
    ('P2', 'W premise: lines before the failure allowed', F,
     "    if early:\n        bad.append(f\"the run printed {early[0]['text']!r} before the failure\")\n", ""),
    ('P3', 'U premise: round 1 not needed', F,
     "              [] if r1 and r1['at'] is not None and r1['at'] < fail_real\n", "              [] if True\n"),
    ('P4', 'zed premise: not compared', F, "want in ('on', 'off') and want == runs", "want in ('on', 'off')"),
    ('P5', 'refused-in-time premise always holds', F,
     "              [] if r3 and began(r3) is not None and c['refused_at'] < began(r3)\n", "              [] if True\n"),
    ('P6', 'served-in-time premise always holds', F,
     "              [] if r4 and began(r4) is not None and c['served_at'] < began(r4)\n", "              [] if True\n"),
    # Reading and the course.
    ('R1', 'a line of no kind skipped', F,
     "            raise RigError(f\"a line of the run's output that is not a round, lost or outcome line: {ln!r}\")",
     "            continue"),
    ('R2', "the recorder's verdicts not read", F,
     "            j.holds(f'pin_{cid}_{pin}', [what], 'server', '')", "            pass"),
    ('R3', 'a verdict beside an error is 2', F, '        return 1 if self.failed else self.rc', '        return self.rc or (1 if self.failed else 0)'),
    ('R4', "a case's rig error judged anyway", F,
     "            j.premise(f'premise_{cid}_rig', [f\"the rig's own error in the case: {cerr}\"], '')\n            continue",
     "            j.premise(f'premise_{cid}_rig', [f\"the rig's own error in the case: {cerr}\"], '')"),
    ('R5', 'the course-wide rig error not read', F, "        j.rc = max(j.rc, 2)\n        j.put('rig_error_in_the_course'",
     "        j.put('rig_error_in_the_course'"),
    ('R6', 'agent_check: no hand-off', A, "    if sys.argv[1] in agent_fail_check.COURSES:\n        sys.exit(agent_fail_check.main(sys.argv[1:3]))\n", ""),
    ('R7', 'another course accepted', F, "        if name not in COURSES:\n", "        if False:\n"),
    ('R8', 'a lost line with any verdict word read', F, r"looked_ms=([0-9]+|-) verdict=(lost|failed|retry)')",
     r"looked_ms=([0-9]+|-) verdict=(\S+)')"),
]


def main():
    tree, out = sys.argv[1], sys.argv[2]
    with open(out, 'w') as o:
        o.write('id\tresult\tdetail\twhat\n')
        base = subprocess.run([sys.executable, 'tests/kernel/test_agent_fail_check.py'], cwd=tree,
                              capture_output=True, text=True, timeout=600)
        o.write(f"BASE\t{'passes' if base.returncode == 0 else 'FAILS'}\t{base.stderr.strip().splitlines()[-1]}\t-\n")
        o.flush()
        for pid, what, path, old, new in PLANTS:
            p = os.path.join(tree, path)
            with open(p) as fh:
                text = fh.read()
            n = text.count(old)
            if n != 1:
                o.write(f'{pid}\tnot-applied\t{n} matches\t{what}\n')
                o.flush()
                continue
            with open(p, 'w') as fh:
                fh.write(text.replace(old, new))
            try:
                r = subprocess.run([sys.executable, 'tests/kernel/test_agent_fail_check.py'], cwd=tree,
                                   capture_output=True, text=True, timeout=600)
                fails = re.findall(r'^(?:FAIL|ERROR): (\S+) \(', r.stderr, re.M)
                res = 'killed' if r.returncode != 0 else 'SURVIVES'
                detail = f"{len(fails)} failing: {', '.join(sorted(set(fails))[:3])}" if fails else '-'
            except subprocess.TimeoutExpired:
                res, detail = 'timeout', '-'
            finally:
                subprocess.run(['git', 'checkout', '--', path], cwd=tree, check=True)
            o.write(f'{pid}\t{res}\t{detail}\t{what}\n')
            o.flush()
        st = subprocess.run(['git', 'status', '--porcelain'], cwd=tree, capture_output=True, text=True).stdout
        o.write(f"END\t{'clean' if not st.strip() else 'DIRTY'}\t-\t-\n")


if __name__ == '__main__':
    main()
