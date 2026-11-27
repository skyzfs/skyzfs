#!/usr/bin/env python3
"""Plants for the review of skyzfs/skyzfs#178 at b7202e1.

Each plant is one textual change to one file of the checkout given as
argv[1] (a worktree at the head). The runner applies it alone, runs the
tests named, records whether each test command failed (the plant is
caught) or passed (it survives), and restores the file with
`git checkout -- FILE` before the next one. Every command runs under
`timeout 600`.

Usage: plants.py CHECKOUT OUT.json [ID...]
"""
import json
import os
import subprocess
import sys

PC = 'tests/kernel/publish_check.py'
MAIN = 'tests/kernel/test_publish_check_main.py'
TAB = 'tests/kernel/test_publish_check.py'
AG = 'tests/kernel/publish_agent.py'
TAG = 'tests/kernel/test_publish_agent.py'
SH = 'tests/kernel/cut-record.sh'
TSH = 'tests/kernel/test_cut_record.sh'

T_TAB = 'python3 tests/kernel/test_publish_check.py'
T_MAIN = 'python3 tests/kernel/test_publish_check_main.py'
T_AG = 'python3 tests/kernel/test_publish_agent.py'
T_SH = 'bash tests/kernel/test_cut_record.sh'

CHECK = [T_TAB, T_MAIN]

# (id, what it breaks, file, old, new, tests)
PLANTS = [
    # --- publish_check.py: RETIRES ---
    ('R1', "RETIRES pretire without e9's `gone`", PC,
     r"""RETIRES = {'pretire': [[('e1', 'refused other'), ('e1', 'retired'), ('e1', 'gone'), ('e9', 'gone')]],""",
     r"""RETIRES = {'pretire': [[('e1', 'refused other'), ('e1', 'retired'), ('e1', 'gone')]],""", CHECK),
    ('R2', "RETIRES pretire: e1's second line `gone`, not `retired`", PC,
     r"""RETIRES = {'pretire': [[('e1', 'refused other'), ('e1', 'retired'), ('e1', 'gone'), ('e9', 'gone')]],""",
     r"""RETIRES = {'pretire': [[('e1', 'refused other'), ('e1', 'gone'), ('e1', 'gone'), ('e9', 'gone')]],""", CHECK),
    ('R3', 'RETIRES pkillret: the killed instance wrote `k1 retired`', PC,
     r"""           'pkillret': [[], [('k1', 'gone')]]}""",
     r"""           'pkillret': [[('k1', 'retired')], [('k1', 'gone')]]}""", CHECK),
    ('R4', 'RETIRES pkillret: one instance only', PC,
     r"""           'pkillret': [[], [('k1', 'gone')]]}""",
     r"""           'pkillret': [[('k1', 'gone')]]}""", CHECK),
    ('R5', 'judge_retire_lines: order not judged', PC,
     r"""    return [f'instance {k + 1}: {s or "none"}, not {w or "none"}' for k, (s, w) in enumerate(zip(seen, want))
            if s != w]""",
     r"""    return [f'instance {k + 1}: {s or "none"}, not {w or "none"}' for k, (s, w) in enumerate(zip(seen, want))
            if sorted(s) != sorted(w)]""", CHECK),
    ('R6', 'judge_retire_lines: the count of instances not judged', PC,
     r"""    if len(seen) != len(want):
        return [f'{len(seen)} instances in the request log, not {len(want)}']
""", '', CHECK),
    ('R7', 'retire lines judged for pretire only', PC,
     "        if name in RETIRES:\n", "        if name == 'pretire':\n", CHECK),
    # --- ABSENT ---
    ('A1', 'ABSENT pretire without e1', PC,
     r"""'passist': ('a2',), 'pretire': ('e1', 'e9'), 'pkillret': ('k1',)}""",
     r"""'passist': ('a2',), 'pretire': ('e9',), 'pkillret': ('k1',)}""", CHECK),
    ('A2', 'ABSENT pretire without e9', PC,
     r"""'passist': ('a2',), 'pretire': ('e1', 'e9'), 'pkillret': ('k1',)}""",
     r"""'passist': ('a2',), 'pretire': ('e1',), 'pkillret': ('k1',)}""", CHECK),
    ('A3', 'ABSENT pkillret empty', PC,
     r"""'passist': ('a2',), 'pretire': ('e1', 'e9'), 'pkillret': ('k1',)}""",
     r"""'passist': ('a2',), 'pretire': ('e1', 'e9'), 'pkillret': ()}""", CHECK),
    ('A4', 'ABSENT judges the ref only (a row left is not seen)', PC,
     r"""            holds(f'pin_{t}_absent', [w for w, there in (('a ref snap/T', got['ref'] is not None),
                                                          ('a row', got['row'] is not None)) if there],""",
     r"""            holds(f'pin_{t}_absent', [w for w, there in (('a ref snap/T', got['ref'] is not None),
                                                          ('a row', False)) if there],""", CHECK),
    # --- the premise of pkillret ---
    ('K1', 'pkillret premise: the ref only', PC,
     r"""premise('premise_k1_retired_before_the_kill', got['ref'] is None and got['row'] is None,""",
     r"""premise('premise_k1_retired_before_the_kill', got['ref'] is None,""", CHECK),
    ('K2', 'pkillret premise: the row only', PC,
     r"""premise('premise_k1_retired_before_the_kill', got['ref'] is None and got['row'] is None,""",
     r"""premise('premise_k1_retired_before_the_kill', got['row'] is None,""", CHECK),
    ('K3', 'pkillret premise: always holds', PC,
     r"""premise('premise_k1_retired_before_the_kill', got['ref'] is None and got['row'] is None,""",
     r"""premise('premise_k1_retired_before_the_kill', True,""", CHECK),
    ('K4', 'pkillret not in KILLED (the hook premise not read)', PC,
     r"""KILLED = {'pkillman': 'm1', 'pkillpub': 'p1', 'pkillret': 'k1'}""",
     r"""KILLED = {'pkillman': 'm1', 'pkillpub': 'p1'}""", CHECK),
    ('K5', "KILLED_BY pkillret: the hook's words without the name", PC,
     r"""             'pkillret': 'after the transaction of retire 1 (k1) landed'}""",
     r"""             'pkillret': 'after the transaction of retire'}""", CHECK),
    # --- EXPECTED ---
    ('E1', "EXPECTED e1-other: any refusal", PC,
     r"""        'e1-other': ('refused other',),       # e2's commit: snap/e1 names another""",
     r"""        'e1-other': ('refused ',),       # e2's commit: snap/e1 names another""", CHECK),
    ('E2', 'EXPECTED e1-kept: absent too', PC,
     r"""        'e1-kept': ('snap ',),                # and is left as it was""",
     r"""        'e1-kept': ('snap ', 'absent'),                # and is left as it was""", CHECK),
    ('E3', 'EXPECTED e1-retire: gone too', PC,
     r"""        'e1-retire': ('retired',),""", r"""        'e1-retire': ('retired', 'gone'),""", CHECK),
    ('E4', 'EXPECTED e1-look: snap too', PC,
     r"""        'e1-look': ('absent',),""", r"""        'e1-look': ('absent', 'snap '),""", CHECK),
    ('E5', 'EXPECTED e1-again: retired too', PC,
     r"""        'e1-again': ('gone',),                # a retire that landed, asked again""",
     r"""        'e1-again': ('gone', 'retired'),                # a retire that landed, asked again""", CHECK),
    ('E6', 'EXPECTED e9: any refusal too', PC,
     r"""        'e9': ('gone',),                      # never published""",
     r"""        'e9': ('gone', 'refused'),                      # never published""", CHECK),
    ('E7', 'EXPECTED k1-retire: retired too', PC,
     r"""        'k1-retire': ('none ',),""", r"""        'k1-retire': ('none ', 'retired'),""", CHECK),
    ('E8', 'EXPECTED k1-again: retired too', PC,
     r"""        'k1-again': ('gone',),""", r"""        'k1-again': ('gone', 'retired'),""", CHECK),
    ('E9', 'EXPECTED k1-look: snap too', PC,
     r"""        'k1-look': ('absent',),
    },
    'pchannel': {""", r"""        'k1-look': ('absent', 'snap '),
    },
    'pchannel': {""", CHECK),
    ('E10', 'PUBLISHED pretire without e2', PC,
     r"""    'pretire': {'e2': 'e2'},                  # e1 is retired, and e2 kept""",
     r"""    'pretire': {},                  # e1 is retired, and e2 kept""", CHECK),
    # --- the pins of pretire ---
    ('P1', 'pin_e1_other: the commit among others', PC,
     "other == [commit] else", "commit in other else", CHECK),
    ('P2', 'pin_e1_other: any commit', PC,
     "other == [commit] else", "bool(other) else", CHECK),
    ('P3', 'pin_e1_other: no word boundary', PC,
     r"""re.findall(r'\b[0-9a-f]{64}\b', ends.get('e1-other', ''))""",
     r"""re.findall(r'[0-9a-f]{64}', ends.get('e1-other', ''))""", CHECK),
    ('P4', "pin_e1_kept: any `snap` answer", PC,
     "kept.startswith(f'snap commit={commit} ')", "kept.startswith('snap ')", CHECK),
    ('P5', 'judge_listed: a listed absent name not judged', PC,
     "    bad = [f'{n} is listed (on {refs[n][:12]})' for n in absent if n in refs]\n",
     "    bad = []\n", CHECK),
    ('P6', "judge_listed: e2's commit not judged", PC,
     "        elif c is not None and refs[n] != c:\n", "        elif False:\n", CHECK),
    ('P7', 'judge_listed: e2 not listed not judged', PC,
     "        if n not in refs:\n            bad.append(f'{n} is not listed')\n        elif",
     "        if n not in refs:\n            pass\n        elif", CHECK),
    ('P8', "pin_e1_not_listed: e2 on any commit", PC,
     "{'snap/e2': done_commit(ends.get('e2', ''))}", "{'snap/e2': None}", CHECK),
    # --- the fixtures of test_publish_check_main.py: a daemon that misbehaves ---
    ('F1', "fixture: e1's retire answered `gone` (nothing deleted, said gone)", MAIN,
     r"""'e1-kept': look_of('e1', 8, 50), 'e1-retire': 'retired', 'e1-look'""",
     r"""'e1-kept': look_of('e1', 8, 50), 'e1-retire': 'gone', 'e1-look'""", [T_MAIN]),
    ('F2', 'fixture: a look reads e1 after its retire', MAIN,
     r"""'e1-retire': 'retired', 'e1-look': 'absent', 'e1-again'""",
     r"""'e1-retire': 'retired', 'e1-look': look_of('e1', 8, 50), 'e1-again'""", [T_MAIN]),
    ('F3', 'fixture: node B still finds e1 (ref and row)', MAIN,
     r"""        'rows': {'e2': (9, 51)},""", r"""        'rows': {'e1': (8, 50), 'e2': (9, 51)},""", [T_MAIN]),
    ('F4', "fixture: node B's listing has snap/e1", MAIN,
     r"""        'files': {'refs-after.txt': f'head\t{"f" * 64}\t9\nsnap/e2\t{commit_of("e2")}\t3\n'},""",
     r"""        'files': {'refs-after.txt': f'head\t{"f" * 64}\t9\nsnap/e1\t{commit_of("e1")}\t3\nsnap/e2\t{commit_of("e2")}\t3\n'},""",
     [T_MAIN]),
    ('F5', "fixture: the request log lacks `e1 retired`", MAIN,
     r"""                       ('publish', 'retire', 'e1 retired'), ('publish', 'retire', 'e1 gone'),""",
     r"""                       ('publish', 'retire', 'e1 gone'),""", [T_MAIN]),
    ('F6', "fixture: `other` names e2's commit (the request's)", MAIN,
     r"""'e1-other': f"refused other snap/T names commit {commit_of('e1')}, not""",
     r"""'e1-other': f"refused other snap/T names commit {commit_of('e2')}, not""", [T_MAIN]),
    ('F7', "fixture: the refusal deleted e1 (look absent)", MAIN,
     r"""                 'e1-kept': look_of('e1', 8, 50), 'e1-retire'""",
     r"""                 'e1-kept': 'absent', 'e1-retire'""", [T_MAIN]),
    ('F8', "fixture: e9, never published, answered `retired`", MAIN,
     r"""                 'e9': 'gone'},""", r"""                 'e9': 'retired'},""", [T_MAIN]),
    ('F9', "fixture: e1 asked again answered `retired`", MAIN,
     r"""'e1-look': 'absent', 'e1-again': 'gone',""", r"""'e1-look': 'absent', 'e1-again': 'retired',""", [T_MAIN]),
    ('F10', 'fixture: snap/k1 there after the kill (ref and row)', MAIN,
     r"""                  'k1-between.txt': 'ref\tnone\nsnaprow\tnone\n'},""",
     r"""                  'k1-between.txt': f'ref\t{commit_of("k1")}\t3\nsnaprow\t{S}\t8\t50\t{commit_of("k1")}\t{ULID}\t1\t{"0" * 64}\n'},""",
     [T_MAIN]),
    ('F11', 'fixture: a torn retire at the kill (row left)', MAIN,
     r"""                  'k1-between.txt': 'ref\tnone\nsnaprow\tnone\n'},""",
     r"""                  'k1-between.txt': f'ref\tnone\nsnaprow\t{S}\t8\t50\t{commit_of("k1")}\t{ULID}\t1\t{"0" * 64}\n'},""",
     [T_MAIN]),
    ('F12', "fixture: the killed instance logged `k1 retired`", MAIN,
     r"""        'log': [(PID, [(8, 50, True)]), (PID2, [('publish', 'retire', 'k1 gone')])],""",
     r"""        'log': [(PID, [(8, 50, True), ('publish', 'retire', 'k1 retired')]), (PID2, [('publish', 'retire', 'k1 gone')])],""",
     [T_MAIN]),
    ('F13', 'fixture: the next instance retired k1 again', MAIN,
     r"""'k1-retire': 'none (exit 2)', 'k1-again': 'gone',""", r"""'k1-retire': 'none (exit 2)', 'k1-again': 'retired',""",
     [T_MAIN]),
    ('F14', 'fixture: the killed retire answered', MAIN,
     r"""'k1-retire': 'none (exit 2)',""", r"""'k1-retire': 'retired',""", [T_MAIN]),
    ('F15', 'fixture: the daemon exited 0, not killed', MAIN,
     r"""        'files': {'kill.tsv': 'killed_rc\t137\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the transaction of retire 1 (k1) '""",
     r"""        'files': {'kill.tsv': 'killed_rc\t0\n',
                  'serve.log': 'skyzfsd: test hook --kill-at-upload: killed after the transaction of retire 1 (k1) '""",
     [T_MAIN]),
    ('F16', 'fixture: node B finds k1 after the take-over', MAIN,
     r"""        'requests': {'k1': (7, 50, 50)},
        'rows': {},""", r"""        'requests': {'k1': (7, 50, 50)},
        'rows': {'k1': (8, 50)},""", [T_MAIN]),
    # --- publish_agent.py ---
    ('G1', 'is_number is isdigit', AG,
     "    return text.isascii() and text.isdigit()\n", "    return text.isdigit()\n", [T_AG]),
    ('G2', 'rewound alone by isdigit', AG,
     "        if not is_number(fields['rewound']):", "        if not fields['rewound'].isdigit():", [T_AG]),
    ('G3', 'count alone by isdigit', AG,
     "        if not is_number(fields['count']):", "        if not fields['count'].isdigit():", [T_AG]),
    ('G4', 'the cuts group alone by isdigit', AG,
     "fields['group'] != 'none' and (not is_number(fields['group'])",
     "fields['group'] != 'none' and (not fields['group'].isdigit()", [T_AG]),
    ('G5', "snap's manifest-len alone by isdigit", AG,
     "and all(is_number(fields[k]) for k in ('number', 'group', 'manifest-len'))):",
     "and all(is_number(fields[k]) for k in ('number', 'group')) and fields['manifest-len'].isdigit()):", [T_AG]),
    ('G6', "snap's group alone by isdigit", AG,
     "and all(is_number(fields[k]) for k in ('number', 'group', 'manifest-len'))):",
     "and all(is_number(fields[k]) for k in ('number', 'manifest-len')) and fields['group'].isdigit()):", [T_AG]),
    ('G7', "`retired` not a known answer", AG,
     "'unreadable': {'commit', 'why'}, 'retired': set(), 'gone': set()}",
     "'unreadable': {'commit', 'why'}, 'gone': set()}", [T_AG]),
    ('G8', "`retired` takes any field", AG,
     "'unreadable': {'commit', 'why'}, 'retired': set(), 'gone': set()}",
     "'unreadable': {'commit', 'why'}, 'retired': {'commit'}, 'gone': set()}", [T_AG]),
    ('G9', 'retire sends --hash', AG,
     "return self.ask('retire', '--name', name, '--commit', commit)",
     "return self.ask('retire', '--name', name, '--hash', commit)", [T_AG]),
    ('G10', "main's retire swaps T and H", AG,
     "                a = agent.retire(args[0], args[1])", "                a = agent.retire(args[1], args[0])", [T_AG]),
    # --- cut-record.sh ---
    ('C1', 'pretire asks e1-other with its own commit', SH,
     'pask e1-other retire e1 "$(pcommit e2)"', 'pask e1-other retire e1 "$(pcommit e1)"', [T_SH]),
    ('C2', 'pkillret kills at the second retire', SH,
     '        pkillret) SERVE_ARGS=(--kill-at-upload retire:1) ;;',
     '        pkillret) SERVE_ARGS=(--kill-at-upload retire:2) ;;', [T_SH]),
    ('C3', 'pkillret not a publish course', SH,
     'PUBLISH_NAMED="pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist pretire pkillret"',
     'PUBLISH_NAMED="pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel passist pretire"', [T_SH]),
    ('C4', 'pcommit takes any done', SH,
     "        | sed -n 's/^done commit=\\([0-9a-f]\\{64\\}\\)$/\\1/p')",
     "        | sed -n 's/^done commit=\\([0-9a-f]*\\)/\\1/p')", [T_SH]),
    ('C5', 'node B read after the take-over', SH,
     '''    nodeb snap-row --snapshot k1 > "$RDIR/k1-between.txt" 2> "$RDIR/k1-between.err" \\
        || rig_error "node B snap-row failed: $(tail -n 2 "$RDIR/k1-between.err")"
    SERVE_ARGS=()
    serve_start 2
''', '''    SERVE_ARGS=()
    serve_start 2
    nodeb snap-row --snapshot k1 > "$RDIR/k1-between.txt" 2> "$RDIR/k1-between.err" \\
        || rig_error "node B snap-row failed: $(tail -n 2 "$RDIR/k1-between.err")"
''', [T_SH]),
    ('C6', 'pkillret does not wait for the daemon to die', SH,
     '''    for _i in $(seq 1 1200); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$SERVE_PID" 2>/dev/null && rig_error "the daemon had not killed itself in its first retire 300 s on"
''', '', [T_SH]),
    ('C7', 'the node B listing taken before the retire', SH,
     '''    pask e1-retire retire e1 "$(pcommit e1)"
    pask e1-look look e1
    nodeb refs > "$RDIR/refs-after.txt" 2> "$RDIR/refs-after.err" \\
        || rig_error "node B refs failed: $(tail -n 2 "$RDIR/refs-after.err")"
''', '''    nodeb refs > "$RDIR/refs-after.txt" 2> "$RDIR/refs-after.err" \\
        || rig_error "node B refs failed: $(tail -n 2 "$RDIR/refs-after.err")"
    pask e1-retire retire e1 "$(pcommit e1)"
    pask e1-look look e1
''', [T_SH]),
]


def run(cmd, cwd):
    p = subprocess.run(['timeout', '600'] + cmd.split(), cwd=cwd, capture_output=True, text=True)
    tail = (p.stdout + p.stderr).strip().splitlines()[-3:]
    return p.returncode, tail


def main():
    co, out = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    res = []
    for pid, what, fn, old, new, tests in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(co, fn)
        with open(path) as f:
            text = f.read()
        n = text.count(old)
        if n != 1:
            res.append({'id': pid, 'what': what, 'error': f'old occurs {n} times'})
            print(pid, 'ERROR: old occurs', n, 'times', flush=True)
            continue
        with open(path, 'w') as f:
            f.write(text.replace(old, new))
        try:
            got = {}
            for t in tests:
                rc, tail = run(t, co)
                got[t] = {'rc': rc, 'tail': tail}
        finally:
            subprocess.run(['git', 'checkout', '--', fn], cwd=co, check=True)
        caught = [t for t in tests if got[t]['rc'] != 0]
        res.append({'id': pid, 'what': what, 'file': fn, 'tests': got, 'caught_by': caught})
        print(pid, 'CAUGHT by ' + ', '.join(caught) if caught else 'SURVIVES', '|', what, flush=True)
    clean = subprocess.run(['git', 'status', '--porcelain'], cwd=co, capture_output=True, text=True).stdout
    res.append({'clean_after': clean == ''})
    with open(out, 'w') as f:
        json.dump(res, f, indent=1)
    print('clean after:', clean == '')


if __name__ == '__main__':
    main()
