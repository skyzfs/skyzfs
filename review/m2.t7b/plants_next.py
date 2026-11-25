#!/usr/bin/env python3
"""Next look at #141 at b76ab97: textual plants in the publish rig, one at
a time, each file put back in a finally, against the pull request's four
suites (test_publish_check.py, test_publish_check_main.py,
test_publish_agent.py, test_cut_record.sh), 600 s each. KILLED: a suite
failed; SURVIVED: all passed; NOT APPLIED: the text to change is not in
the file exactly once.

- L1 to L13: the pull request's own plants (its description), rebuilt
  from the words; L2, L5, L6 and L13 on the code b76ab97 rewrote (the
  text of e0c51f8 no longer exists).
- R1 to R49: this review's plants at e0c51f8 (plants141.py), as they
  were where their text still exists, and re-aimed (an `x` suffix) where
  6647d4b or b76ab97 rewrote it.
- M1 to M3, P1 to P9: testing's plants of 6647d4b and b76ab97, rebuilt
  from its answer's words.
- N1 on: this look's, on the new code.

Usage: python3 -I plants_next.py WORKTREE [ID...]
"""
import json
import os
import subprocess
import sys

WT = sys.argv[1]
ONLY = set(sys.argv[2:])
K = os.path.join(WT, 'tests', 'kernel')
PC, PA, SH = 'publish_check.py', 'publish_agent.py', 'cut-record.sh'
WF = '../../.github/workflows/skyzfs-publish-rig.yml'

PREMF_OLD = ("            premise('premise_f1_import_f', zeroed.isdigit() and int(zeroed) > 0 and\n"
             "                    any(ln in ('imported with -F', 'imported with -FX') for ln in damage.splitlines()),\n")
GROUND = "                if top is not None and number > after and group <= top:\n"

PLANTS = [
    # The pull request's L1 to L13.
    ('L1', PC, "want = {'commit': row['commit'], 'state'", "want = {'state'", 'judge_look without the commit'),
    ('L2x', PC, "ln['event'] == 'publish' and ln['op'] == 'ended' and ln['result'].split(' ', 1)[0] == name]",
     "ln['event'] == 'publish' and ln['op'] in ('ended', 'asked') and ln['result'].split(' ', 1)[0] == name]",
     "prewindf's end read from a refusal when asked as well"),
    ('L3', PC, "look = LOOKS.get(name, {}).get(t)", "look = None", 'the look pins skipped'),
    ('L4', PC, "ABSENT = {'pplain': ('t5', 't3'), 'prewindf': ('f1',), 'pkill': ('tw',)}",
     "ABSENT = {'pplain': ('t5', 't3'), 'pkill': ('tw',)}", 'f1 not in ABSENT'),
    ('L5x', PC, "premise('premise_f1_waited_across_a_rewind_cut', bool(within),",
     "premise('premise_f1_waited_across_a_rewind_cut', True,", "prewindf's premise without its ground"),
    ('L6x', PC, PREMF_OLD, PREMF_OLD.replace("zeroed.isdigit() and int(zeroed) > 0 and\n                    ", ""),
     'the import premise without the damage'),
    ('L7', PC, "'r1-look': ('refused store', 'refused no-answer'),  # object storage down: never `absent`",
     "'r1-look': ('refused store', 'refused no-answer', 'absent'),", 'absent allowed with the store down'),
    ('L8', PC, "'f1-end': ('refused rewound',),", "'f1-end': ('refused rewound', 'pending'),", "pending allowed for f1's end"),
    ('L9', SH, "    pask t1-look look t1\n", "", "t1's look dropped"),
    ('L10', SH, "    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n    export_pool\n",
     "    export_pool\n    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n", 'f1 asked after the export'),
    ('L11', SH, "    pwait f1-end f1 300 sync\n", "    pwait f1-end f1 300\n", "f1's end read with no syncs"),
    ('L12', SH, "    pask r1-look look r1\n    bash -c \"$S3_UP\"", "    bash -c \"$S3_UP\"", "r1's look dropped"),
    ('L13x', SH, "    export_pool\n    zero_newest_root\n    import_rewind_f\n    pwait f1-end",
     "    export_pool\n    import_rewind_f\n    pwait f1-end", 'prewindf: the import without the damage'),
    # This review's, at e0c51f8, re-aimed at rewind_cuts where rewritten.
    ('R1x', PC, GROUND, GROUND.replace('group <= top', 'group < top'),
     "rewind_cuts: a cut at the top's group is no ground (-FX's first cut is)"),
    ('R2x', PC, GROUND, GROUND.replace('number > after', 'number >= after'), 'rewind_cuts: the cut at C counts'),
    ('R3x', PC, GROUND, GROUND.replace('number > after and ', ''), 'rewind_cuts: cuts at or before C count'),
    ('R4x', PC, "return lambda ln: ln['event'] == 'publish' and ln['op'] == 'asked' and",
     "return lambda ln: ln['event'] == 'publish' and", 'asked_line: any op'),
    ('R6x', PC, "ln['op'] == 'asked' and ln['result'].split(' ', 1)[0] == name\n",
     "ln['op'] == 'asked'\n", 'asked_line: any name'),
    ('R7x', PC, "    for inst in instances:\n        top, seen = None, []\n        for ln in inst['lines']:\n            if stop",
     "    top, seen = None, []\n    for inst in instances:\n        for ln in inst['lines']:\n            if stop",
     'rewind_cuts: the top carried across instances'),
    ('R8x', PC, "                top = group if top is None else max(top, group)\n",
     "                top = group\n", 'rewind_cuts: the cut before, not the running maximum'),
    ('R9', PC, "if named is not None and named < number and any(c[0] == named and c[2] for c in cuts):",
     "if named is not None and named < number:", "rewind_committed as before D1's fix"),
    ('R10', PC, "if named is not None and named < number and any(c[0] == named and c[2] for c in cuts):",
     "if False:", 'rewind_committed without its named check'),
    ('R11', PC, "holds('pin_r1_above_top', [] if group is not None and group > int(top) else",
     "holds('pin_r1_above_top', [] if group is not None and group >= int(top) else", 'pin_r1_above_top: at the top passes'),
    ('R12', PC, "            if published['r1'] == r1:\n                raise RigError", "            if False:\n                raise RigError",
     'r1 not asked anew passes'),
    ('R13x', PC, "found, grounds = rewind_cuts(instances, r1['after'], asked_line('r1'))",
     "found, grounds = rewind_cuts(instances, published['r1']['after'], asked_line('r1'))", "prewind's ground from the anew C"),
    ('R14x', PC, "premise('premise_r1_asked_after_a_rewind_cut', found and bool(grounds),",
     "premise('premise_r1_asked_after_a_rewind_cut', True,", "prewind's ground premise always holds"),
    ('R15', PC, "if not isinstance(row, dict):\n        return ['node B read no row to compare the look with']",
     "if not isinstance(row, dict):\n        return []", 'judge_look: no row is no failure'),
    ('R16', PC, "for k, v in want.items() if f.get(k) != v]", "for k, v in want.items() if k in f and f.get(k) != v]",
     'judge_look: a field missing from the look passes'),
    ('R17', PC, "'manifest-len': str(row['manifest_len']), 'manifest-hash': row['manifest_hash']}",
     "'manifest-len': str(row['manifest_len'])}", 'judge_look without the manifest hash'),
    ('R18', PC, "'t5-look': ('absent',),", "'t5-look': ('absent', 'snap '),", 't5-look: a snap allowed'),
    ('R19', PC, "'t3-look': ('absent',),", "'t3-look': ('absent', 'refused '),", 't3-look: a refusal allowed'),
    ('R20', PC, "'f1': ('pending after',),", "'f1': ('pending after', 'refused rewound'),", 'f1: a refusal at the ask allowed'),
    ('R21', PC, "LOOKS = {'pplain': {'t1': 't1-look'}, 'pkillpub': {'p1': 'p1-look'}}", "LOOKS = {'pkillpub': {'p1': 'p1-look'}}",
     "LOOKS without pplain's t1"),
    ('R22x', PC, PREMF_OLD, PREMF_OLD.replace(" and\n                    any(ln in ('imported with -F', 'imported with -FX') "
                                              "for ln in damage.splitlines()),", ","),
     'the import premise without the import'),
    ('R23x', PC, "zeroed.isdigit() and int(zeroed) > 0 and\n", "zeroed.isdigit() and\n",
     'the import premise with nothing zeroed'),
    ('R24', PC, "        if name == 'prewindf':\n", "        if name == 'prewindf-':\n", "prewindf's premises skipped"),
    ('R25', PC, "COURSES = ('pplain', 'prewind', 'prewindf', 'pkill',", "COURSES = ('pplain', 'prewind', 'pkill',",
     'prewindf not a course of the check'),
    ('R26', PC, "            top = text_of('rewind-top.txt').strip()\n            if not top.isdigit():\n                raise RigError",
     "            top = text_of('rewind-top.txt').strip()\n            if not top:\n                raise RigError",
     'rewind-top.txt not checked as digits'),
    ('R30', PA, "if len(whole) > 1 and whole[0] == HELLO:", "if len(whole) > 1:", 'answer_of: no greeting needed (D2 undone)'),
    ('R31', PA, "            except TimeoutError:\n                late = True\n                break\n", "",
     'raw: a timeout is the rig\'s error again (D3 undone)'),
    ('R32', PA, "answer = f'no answer within {limit} s' if late and not got else answer_of(got)",
     "answer = f'no answer within {limit} s' if late else answer_of(got)", 'raw: a whole answer and no close is no answer'),
    ('R33', PA, "answer = f'no answer within {limit} s' if late and not got else answer_of(got)",
     "answer = '' if late and not got else answer_of(got)", "raw: a timeout answers ''"),
    ('R34', PA, "    if verb in ('done', 'snap', 'unreadable') and not is_hex(fields['commit'], 64):",
     "    if verb in ('done', 'snap') and not is_hex(fields['commit'], 64):", "parse: unreadable's commit unchecked (A3)"),
    ('R35', PA, "all(fields[k].isdigit() for k in ('number', 'group', 'manifest-len'))",
     "all(fields[k].isdigit() for k in ('number', 'group'))", 'parse: manifest-len unchecked (A5)'),
    ('R36', PA, "a = {'progress': agent.progress, 'withdraw': agent.withdraw, 'look': agent.look}[req](args[0])",
     "a = {'progress': agent.progress, 'withdraw': agent.withdraw, 'look': agent.progress}[req](args[0])",
     'ask look sends publish-progress (A4)'),
    ('R40', SH, "    pask t5-look look t5\n    pask t3-look look t3\n", "    pask t3-look look t3\n    pask t5-look look t5\n",
     "pplain: t3's look before t5's"),
    ('R41', SH, "    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n",
     "    pflow f1 f1 --group-plus 1000000 --expiry-s 5 --limit-s 3\n", 'prewindf: f1 expires in 5 s'),
    ('R42', SH, "    pwait f1-end f1 300 sync\n", "    pwait f1-end f1 3 sync\n", "prewindf: f1's end read within 3 s"),
    ('R43x', SH, "    for opt in -F -FX; do\n", "    for opt in -FX; do\n", 'import_rewind_f: -FX alone, no -F'),
    ('R44', SH, "    top=$(sed -n 's/.* group=\\([0-9]*\\) .*/\\1/p' <<< \"$out\")",
     "    top=$(sed -n 's/.* count=\\([0-9]*\\) .*/\\1/p' <<< \"$out\")", "prewind's top read from the count"),
    ('R45x', SH, "    load 6\n    export_pool\n    zero_newest_root\n    import_rewind_f\n",
     "    load 6\n    export_pool\n    import_rewind_f\n", 'rewindf without the damage'),
    ('R46', SH, 'PUBLISH_NAMED="pplain prewind prewindf pkill pkillman pkillpub pchannel"',
     'PUBLISH_NAMED="pplain prewind pkill pkillman pkillpub pchannel"', 'prewindf not a course by name'),
    ('R47', SH, "    echo \"$top\" > \"$RDIR/rewind-top.txt\"\n", "    echo 0 > \"$RDIR/rewind-top.txt\"\n",
     'prewind: the top written as 0'),
    ('R48', SH, "    pflow r1 r1 --limit-s 300\n    pask r1-look look r1\n",
     "    pask r1-look look r1\n    pflow r1 r1 --limit-s 300\n", "prewind: r1's look before r1 is asked"),
    ('R49', SH, "    pask r1-look look r1\n    bash -c \"$S3_UP\"",
     "    pask r1-look look r1\n    panew r1-anew r1 record\n    bash -c \"$S3_UP\"",
     "prewind: r1 asked anew before the store comes back (#140's S12)"),
    # testing's M1 to M3 (6647d4b).
    ('M1', SH, "    for opt in -F -FX; do\n", "    for opt in -F; do\n", 'import_rewind_f without -FX'),
    ('M2', SH, "    export_pool\n    zero_newest_root\n    import_rewind_f\n    pwait f1-end",
     "    export_pool\n    import_rewind_f\n    pwait f1-end", 'prewindf: the import without the damage (= L13x)'),
    ('M3', PC, "any(ln in ('imported with -F', 'imported with -FX') for ln in damage.splitlines())",
     "'imported with -F' in damage", 'the import premise by substring'),
    # testing's P1 to P9 (b76ab97).
    ('P1', PC, "found, grounds = rewind_cuts(instances, r1['after'], asked_line('r1'))",
     "found, grounds = rewind_cuts(instances, r1['after'], lambda ln: ln['event'] == 'publish' and "
     "ln['op'] == 'asked' and ln['result'] == 'r1 refused rewound')", 'the anchor read as the refusal again'),
    ('P2', PC, "within = [g for g in grounds if g[3] <= deadline]", "within = grounds", 'the expiry filter dropped'),
    ('P3', PC, "if '--expiry-s' in words[:-1] else '600'", "if False else '3600'", 'the expiry taken as 3600 s'),
    ('P4', PC, "    if first['ns'] <= ns:\n", "    if False:\n", "the end's order unchecked"),
    ('P5', PC, "    if first['result'] != f'{name} refused rewound':\n", "    if False:\n", 'any end accepted'),
    ('P6', PC, "ln['op'] == 'withdraw' and ln['result'].split(' ', 1)[0] == name:",
     "ln['op'] == 'withdraw' and ln['result'] == f'{name} withdrawn':", 'the withdraw line read as withdrawn'),
    ('P7', PC, GROUND, GROUND.replace('group <= top', 'group < top'), '`<` for `<=` (= R1x)'),
    ('P8', PC, GROUND, GROUND.replace('number > after and ', ''), 'C not checked (= R3x)'),
    ('P9', PC, "and ln['op'] == 'ended' and ln['result'].split(' ', 1)[0] == name]",
     "and ln['op'] in ('ended', 'asked') and ln['result'].split(' ', 1)[0] == name]", "an `asked` line taken as an end"),
    # This look's.
    ('N1', PC, "        if stop is None and seen:\n            return False, seen\n", "",
     'rewind_cuts: with no STOP, nothing returned'),
    ('N2', PC, "        if stop is None and seen:\n            return False, seen\n    return False, []",
     "        if stop is None and seen:\n            return False, seen\n    return False, []\n    # x",
     'a no-op (the harness itself)'),
    ('N3', PC, "            if stop is not None and stop(ln):\n                return True, seen\n",
     "            if stop is not None and stop(ln):\n                return True, seen[:1]\n",
     'rewind_cuts: only the first ground kept'),
    ('N4', PC, "        if words[:3] != ['publish', '--name', name]:\n", "        if words[:2] != ['publish', '--name']:\n",
     "ask_deadline: any name's publish"),
    ('N5', PC, "        return int(f[0]) + int(expiry) * 1_000_000_000\n", "        return int(f[1]) + int(expiry) * 1_000_000_000\n",
     "ask_deadline: from the agent's answer, not its send"),
    ('N6', PC, "        return int(f[0]) + int(expiry) * 1_000_000_000\n", "        return int(f[0]) + int(expiry) * 1_000_000\n",
     'ask_deadline: the expiry read as milliseconds'),
    ('N7', PC, "        return int(f[0]) + int(expiry) * 1_000_000_000\n    return None\n",
     "        last = int(f[0]) + int(expiry) * 1_000_000_000\n    return locals().get('last')\n",
     "ask_deadline: the last publish of the name, not the first"),
    ('N8', PC, "within = [g for g in grounds if g[3] <= deadline]", "within = [g for g in grounds if g[3] < deadline]",
     'the expiry filter strict'),
    ('N9', PC, "    if first['ns'] <= ns:\n", "    if first['ns'] < ns:\n", "judge_ended: an end at the ground's ns passes"),
    ('N10', PC, "    first, bad = ended[0], []\n", "    first, bad = ended[-1], []\n", "judge_ended: the last end, not the first"),
    ('N11', PC, "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', within[0][3]), 'server',",
     "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', within[-1][3]), 'server',",
     "the pin anchored on the last ground within the expiry"),
    ('N12', PC, "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', within[0][3]), 'server',",
     "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', 0), 'server',",
     'the pin not anchored on the ground'),
    ('N13', PC, "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', within[0][3]), 'server',",
     "holds('pin_f1_ended_after_the_rewind_cut', judge_ended(instances, 'f1', within[0][3]), 'rig',",
     'the pin said to rest on the rig'),
    ('N14', PC, "            _, grounds = rewind_cuts(instances, f1['after'])\n",
     "            _, grounds = rewind_cuts(instances, f1['after'], lambda ln: ln['event'] == 'publish' and "
     "ln['op'] == 'ended')\n", "prewindf's ground up to f1's end again (B2 undone)"),
    ('N15', PC, "any(ln in ('imported with -F', 'imported with -FX') for ln in damage.splitlines())",
     "any(ln in ('imported with -F',) for ln in damage.splitlines())", 'the import premise refuses -FX'),
    ('N16', PC, "ln['op'] == 'withdraw' and ln['result'].split(' ', 1)[0] == name:",
     "ln['op'] == 'withdraw' and ln['result'].startswith(name):", 'withdrawn_past: a name prefix matches'),
    ('N17', PC, "ln['op'] == 'asked' and ln['result'].split(' ', 1)[0] == name\n",
     "ln['op'] == 'asked' and ln['result'].startswith(name)\n", 'asked_line: a name prefix matches'),
    ('N18', SH, "    echo \"imported with $how\" >> \"$RDIR/rewind-f.txt\"\n",
     "    echo \"imported with -F\" >> \"$RDIR/rewind-f.txt\"\n", 'import_rewind_f: always says -F'),
    ('N19', SH, "        [ -z \"$how\" ] || break\n", "", 'import_rewind_f: -FX tried after -F imported'),
    ('N20', SH, "    echo 1 > /sys/module/zfs/parameters/zfs_dbgmsg_enable\n    session_begin\n    local how=\"\" opt\n",
     "    echo 1 > /sys/module/zfs/parameters/zfs_dbgmsg_enable\n    local how=\"\" opt\n",
     'import_rewind_f: no session begun (the marks)'),
    ('N21', SH, "    echo \"imported with $how\" >> \"$RDIR/rewind-f.txt\"\n    mark \"session-$SESSION-ready\"\n",
     "    echo \"imported with $how\" >> \"$RDIR/rewind-f.txt\"\n", 'import_rewind_f: no ready mark'),
    ('N22', SH, "    zero_newest_root\n    import_rewind_f\n    pwait f1-end f1 300 sync\n",
     "    zero_newest_root\n    import_pool -F\n    pwait f1-end f1 300 sync\n", 'prewindf: import_pool -F again (B1 undone)'),
    ('N23', SH, "    [ -n \"$how\" ] || rig_error \"zpool import -F and -FX failed",
     "    [ -n \"$how\" ] || true \"zpool import -F and -FX failed", 'import_rewind_f: both refused is no error'),
    ('N24', WF, '"${COURSES:-pplain prewind prewindf pkill pkillman pkillpub pchannel}"',
     '"${COURSES:-pplain prewind pkill pkillman pkillpub pchannel}"', "the workflow's default list without prewindf"),
]

SUITES = [['python3', '-I', 'test_publish_check.py'], ['python3', '-I', 'test_publish_check_main.py'],
          ['python3', '-I', 'test_publish_agent.py'], ['bash', 'test_cut_record.sh']]


def run_suites():
    failed = []
    for cmd in SUITES:
        try:
            p = subprocess.run(cmd, cwd=K, capture_output=True, text=True, timeout=600)
            if p.returncode != 0:
                failed.append(cmd[-1])
        except subprocess.TimeoutExpired:
            failed.append(cmd[-1] + ' (timeout)')
    return failed


def main():
    base = run_suites()
    print(f'unplanted: {"all pass" if not base else "FAILS " + ", ".join(base)}', flush=True)
    if base:
        sys.exit(2)
    out = []
    for pid, fn, old, new, what in PLANTS:
        if ONLY and pid not in ONLY:
            continue
        path = os.path.normpath(os.path.join(K, fn))
        with open(path) as f:
            text = f.read()
        if text.count(old) != 1:
            verdict, by = 'NOT APPLIED', [f'{text.count(old)} matches']
        else:
            try:
                with open(path, 'w') as f:
                    f.write(text.replace(old, new, 1))
                by = run_suites()
                verdict = 'KILLED' if by else 'SURVIVED'
            finally:
                with open(path, 'w') as f:
                    f.write(text)
        print(f'{pid}\t{verdict}\t{what}\t{", ".join(by)}', flush=True)
        out.append({'id': pid, 'file': fn, 'what': what, 'verdict': verdict, 'by': by})
    after = run_suites()
    print(f'restored: {"all pass" if not after else "FAILS " + ", ".join(after)}', flush=True)
    name = 'plants_next.json' if not ONLY else 'plants_next_rerun.json'
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), name), 'w') as f:
        json.dump(out, f, indent=1)


if __name__ == '__main__':
    main()
