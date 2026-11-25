#!/usr/bin/env python3
"""Review of #143 at d3b75af: the plants of #141's next look that were NOT
APPLIED at this head because the pull request rewrote their text (L4,
R4x, R13x, R25, R46, N24, and P1 of testing's plants), aimed again at
the text as it stands. One at a time, the file put back in a finally,
each suite under a 600 s limit.

Usage: python3 -I reaim143.py WORKTREE
"""
import os
import subprocess
import sys

WT = sys.argv[1]
PC = 'tests/kernel/publish_check.py'
SH = 'tests/kernel/cut-record.sh'
WF = '.github/workflows/skyzfs-publish-rig.yml'
T_PC = [['python3', '-I', 'tests/kernel/test_publish_check.py'],
        ['python3', '-I', 'tests/kernel/test_publish_check_main.py']]
T_SH = [['bash', 'tests/kernel/test_cut_record.sh']]
SUITES = {PC: T_PC, SH: T_SH, WF: T_SH}
PLANTS = [
    ('L4r', PC, "f1 not in ABSENT",
     "ABSENT = {'pplain': ('t5', 't3'), 'prewindf': ('f1',), 'prewindc': ('c1',), 'pkill': ('tw',)}",
     "ABSENT = {'pplain': ('t5', 't3'), 'prewindc': ('c1',), 'pkill': ('tw',)}"),
    ('R4r', PC, "asked_line: any op",
     "return lambda ln: (ln['event'] == 'publish' and ln['op'] == 'asked' and",
     "return lambda ln: (ln['event'] == 'publish' and"),
    ('R13r', PC, "prewind's ground from the anew C",
     "found, grounds = rewind_cuts(instances, r1['after'], asked_line('r1', r1['after']))",
     "found, grounds = rewind_cuts(instances, published['r1']['after'], asked_line('r1', published['r1']['after']))"),
    ('R25r', PC, "prewindf not a course of the check",
     "COURSES = ('pplain', 'prewind', 'prewindf', 'prewindc',", "COURSES = ('pplain', 'prewind', 'prewindc',"),
    ('P1r', PC, "the anchor read as the refusal again",
     "found, grounds = rewind_cuts(instances, r1['after'], asked_line('r1', r1['after']))",
     "found, grounds = rewind_cuts(instances, r1['after'], lambda ln: ln['event'] == 'publish' and "
     "ln['op'] == 'asked' and ln['result'] == 'r1 refused rewound')"),
    ('R46r', SH, "prewindf not a course by name",
     'PUBLISH_NAMED="pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel"',
     'PUBLISH_NAMED="pplain prewind prewindc pkill pkillman pkillpub pchannel"'),
    ('N24r', WF, "the workflow's default list without prewindf",
     '"${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel}"',
     '"${COURSES:-pplain prewind prewindc pkill pkillman pkillpub pchannel}"'),
]


def suites(fn):
    bad = []
    for cmd in SUITES[fn]:
        try:
            if subprocess.run(cmd, cwd=WT, capture_output=True, text=True, timeout=600).returncode:
                bad.append(os.path.basename(cmd[-1]))
        except subprocess.TimeoutExpired:
            bad.append(os.path.basename(cmd[-1]) + ' (TIMEOUT)')
    return bad


print('baseline', {fn: suites(fn) or 'all pass' for fn in (PC, SH)}, flush=True)
for pid, fn, what, old, new in PLANTS:
    path = os.path.join(WT, fn)
    orig = open(path).read()
    if orig.count(old) != 1:
        print(f'{pid}\tNOT APPLIED\t{what}\t{orig.count(old)} matches', flush=True)
        continue
    try:
        open(path, 'w').write(orig.replace(old, new))
        bad = suites(fn)
    finally:
        open(path, 'w').write(orig)
    print(f"{pid}\t{'KILLED' if bad else 'SURVIVED'}\t{what}\t{', '.join(bad)}", flush=True)
dirty = subprocess.run(['git', 'status', '--porcelain'], cwd=WT, capture_output=True, text=True).stdout
print('tree clean' if not dirty else 'TREE DIRTY')
