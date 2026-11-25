#!/usr/bin/env python3
"""Review of #141 at e0c51f8: textual plants in the publish rig, one at a
time, each file put back in a finally. Each plant is run against the
pull request's four suites (test_publish_check.py,
test_publish_check_main.py, test_publish_agent.py, test_cut_record.sh),
600 s each. KILLED: a suite failed; SURVIVED: all passed; NOT APPLIED:
the text to change is not in the file exactly once.

L1 to L13 are the pull request's own plants, as its description names
them (rebuilt here from the words); R1 on are this review's.

Usage: python3 -I plants141.py WORKTREE [ID...]
"""
import json
import os
import subprocess
import sys

WT = sys.argv[1]
ONLY = set(sys.argv[2:])
K = os.path.join(WT, 'tests', 'kernel')
PC, PA, SH = 'publish_check.py', 'publish_agent.py', 'cut-record.sh'

PLANTS = [
    # The pull request's (its description's L1 to L13).
    ('L1', PC, "want = {'commit': row['commit'], 'state'", "want = {'state'", 'judge_look without the commit'),
    ('L2', PC, "rewound_grounds(instances, 'f1', f1['after'], 'ended')", "rewound_grounds(instances, 'f1', f1['after'])",
     "prewindf's end read as a refusal when asked"),
    ('L3', PC, "look = LOOKS.get(name, {}).get(t)", "look = None", 'the look pins skipped'),
    ('L4', PC, "ABSENT = {'pplain': ('t5', 't3'), 'prewindf': ('f1',), 'pkill': ('tw',)}",
     "ABSENT = {'pplain': ('t5', 't3'), 'pkill': ('tw',)}", 'f1 not in ABSENT'),
    ('L5', PC, "premise('premise_f1_ended_by_a_rewind_cut', found and bool(grounds),",
     "premise('premise_f1_ended_by_a_rewind_cut', found,", "prewindf's premise without its ground"),
    ('L6', PC, "zeroed.isdigit() and int(zeroed) > 0 and 'imported with -F' in damage",
     "'imported with -F' in damage", 'the -F premise without the damage'),
    ('L7', PC, "'r1-look': ('refused store', 'refused no-answer'),  # object storage down: never `absent`",
     "'r1-look': ('refused store', 'refused no-answer', 'absent'),", 'absent allowed with the store down'),
    ('L8', PC, "'f1-end': ('refused rewound',),", "'f1-end': ('refused rewound', 'pending'),", "pending allowed for f1's end"),
    ('L9', SH, "    pask t1-look look t1\n", "", "t1's look dropped"),
    ('L10', SH, "    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n    export_pool\n",
     "    export_pool\n    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n", 'f1 asked after the export'),
    ('L11', SH, "    pwait f1-end f1 300 sync\n", "    pwait f1-end f1 300\n", "f1's end read with no syncs"),
    ('L12', SH, "    pask r1-look look r1\n    bash -c \"$S3_UP\"", "    bash -c \"$S3_UP\"", "r1's look after the store is back (step 1: drop)"),
    ('L13', SH, "    export_pool\n    zero_newest_root\n    import_pool -F\n", "    export_pool\n    import_pool -F\n",
     'the import without the damage'),
    # This review's: the check.
    ('R1', PC, "if top is not None and number > after and group <= top:", "if top is not None and number > after and group < top:",
     "rewound_grounds: a cut at the top's group is no ground (-F's first cut is)"),
    ('R2', PC, "if top is not None and number > after and group <= top:", "if top is not None and number >= after and group <= top:",
     'rewound_grounds: the cut at C counts'),
    ('R3', PC, "if top is not None and number > after and group <= top:", "if top is not None and group <= top:",
     'rewound_grounds: cuts at or before C count'),
    ('R4', PC, "elif ln['event'] == 'publish' and ln['op'] == op and \\\n", "elif ln['event'] == 'publish' and \\\n",
     'rewound_grounds: any op'),
    ('R5', PC, "ln['result'].startswith(f'{name} refused rewound')", "ln['result'].startswith(f'{name} refused')",
     'rewound_grounds: any refusal'),
    ('R6', PC, "ln['result'].startswith(f'{name} refused rewound')", "ln['result'].endswith('refused rewound')",
     'rewound_grounds: any name'),
    ('R7', PC, "    for inst in instances:\n        top, seen = None, []\n", "    top, seen = None, []\n    for inst in instances:\n",
     'rewound_grounds: the top carried across instances'),
    ('R8', PC, "top = group if top is None else max(top, group)", "top = group",
     'rewound_grounds: the cut before, not the running maximum (REWIND_BY_PREVIOUS)'),
    ('R9', PC, "if named is not None and named < number and any(c[0] == named and c[2] for c in cuts):",
     "if named is not None and named < number:", "rewind_committed as before D1's fix"),
    ('R10', PC, "if named is not None and named < number and any(c[0] == named and c[2] for c in cuts):",
     "if False:", 'rewind_committed without its named check'),
    ('R11', PC, "holds('pin_r1_above_top', [] if group is not None and group > int(top) else",
     "holds('pin_r1_above_top', [] if group is not None and group >= int(top) else", 'pin_r1_above_top: at the top passes'),
    ('R12', PC, "            if published['r1'] == r1:\n                raise RigError", "            if False:\n                raise RigError",
     'r1 not asked anew passes'),
    ('R13', PC, "found, grounds = rewound_grounds(instances, 'r1', r1['after'])",
     "found, grounds = rewound_grounds(instances, 'r1', published['r1']['after'])", "prewind's ground from the anew C"),
    ('R14', PC, "premise('premise_r1_rewound_with_a_rewind_cut', found and bool(grounds),",
     "premise('premise_r1_rewound_with_a_rewind_cut', True,", "prewind's ground premise always holds"),
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
    ('R22', PC, "zeroed.isdigit() and int(zeroed) > 0 and 'imported with -F' in damage",
     "zeroed.isdigit() and int(zeroed) > 0", 'the -F premise without the import'),
    ('R23', PC, "zeroed.isdigit() and int(zeroed) > 0 and 'imported with -F' in damage",
     "zeroed.isdigit() and 'imported with -F' in damage", 'the -F premise with nothing zeroed'),
    ('R24', PC, "        if name == 'prewindf':\n", "        if name == 'prewindf-':\n", "prewindf's premises skipped"),
    ('R25', PC, "COURSES = ('pplain', 'prewind', 'prewindf', 'pkill',", "COURSES = ('pplain', 'prewind', 'pkill',",
     'prewindf not a course of the check'),
    ('R26', PC, "            top = text_of('rewind-top.txt').strip()\n            if not top.isdigit():\n                raise RigError",
     "            top = text_of('rewind-top.txt').strip()\n            if not top:\n                raise RigError",
     'rewind-top.txt not checked as digits'),
    # The agent.
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
    # The course script.
    ('R40', SH, "    pask t5-look look t5\n    pask t3-look look t3\n", "    pask t3-look look t3\n    pask t5-look look t5\n",
     "pplain: t3's look before t5's"),
    ('R41', SH, "    pflow f1 f1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n",
     "    pflow f1 f1 --group-plus 1000000 --expiry-s 5 --limit-s 3\n", 'prewindf: f1 expires in 5 s'),
    ('R42', SH, "    pwait f1-end f1 300 sync\n", "    pwait f1-end f1 3 sync\n", "prewindf: f1's end read within 3 s"),
    ('R43', SH, "    import_pool -F\n    echo \"imported with -F\"", "    import_pool\n    echo \"imported with -F\"",
     'prewindf: the import without -F'),
    ('R44', SH, "    top=$(sed -n 's/.* group=\\([0-9]*\\) .*/\\1/p' <<< \"$out\")",
     "    top=$(sed -n 's/.* count=\\([0-9]*\\) .*/\\1/p' <<< \"$out\")", "prewind's top read from the count"),
    ('R45', SH, "    export_pool\n    zero_newest_root\n    # What zpool import sees", "    export_pool\n    # What zpool import sees",
     'rewindf without the damage'),
    ('R49', SH, "    pask r1-look look r1\n    bash -c \"$S3_UP\"",
     "    pask r1-look look r1\n    panew r1-anew r1 record\n    bash -c \"$S3_UP\"",
     "prewind: r1 asked anew before the store comes back (#140's S12)"),
    ('R46', SH, 'PUBLISH_NAMED="pplain prewind prewindf pkill pkillman pkillpub pchannel"',
     'PUBLISH_NAMED="pplain prewind pkill pkillman pkillpub pchannel"', 'prewindf not a course by name'),
    ('R47', SH, "    echo \"$top\" > \"$RDIR/rewind-top.txt\"\n", "    echo 0 > \"$RDIR/rewind-top.txt\"\n",
     'prewind: the top written as 0'),
    ('R48', SH, "    pflow r1 r1 --limit-s 300\n    pask r1-look look r1\n",
     "    pask r1-look look r1\n    pflow r1 r1 --limit-s 300\n", "prewind: r1's look before r1 is asked"),
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
    print(f'unplanted: {"all pass" if not base else "FAILS " + ", ".join(base)}')
    if base:
        sys.exit(2)
    out = []
    for pid, fn, old, new, what in PLANTS:
        if ONLY and pid not in ONLY:
            continue
        path = os.path.join(K, fn)
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
    print(f'restored: {"all pass" if not after else "FAILS " + ", ".join(after)}')
    name = 'plants141.json' if not ONLY else 'plants141_rerun.json'
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), name), 'w') as f:
        json.dump(out, f, indent=1)


if __name__ == '__main__':
    main()
