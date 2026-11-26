F = 'tests/kernel/agent_check.py'
T = 'python3 -I tests/kernel/test_agent_check.py'
# The plants that survived the review of 0604977, redone at c580c72.
P = [
 ('P02 killed_at any kill line', "    if lines != [(e, s, str(n))]:", "    if not lines:"),
 ('P13 EXPECTED restart None', "'restart': 'published',", "'restart': None,"),
 ('P14 EXPECTED dhook None', "'dhook': 'published',", "'dhook': None,"),
 ('P17 EXPECTED late None', "'late': 'published',", "'late': None,"),
 ('P18 EXPECTED both None', "'both': 'published',", "'both': None,"),
 ('P20 EXPECTED assist None', "'assist': 'published',", "'assist': None,"),
 ('P43 unmarked stray', "([] if HOLD_TAG in stray else ['the stray hold was released'])", "[]"),
 ('P46 criterion retry any', "elif retry and all(n in pub or n in gone for n in retry):", "elif retry:"),
 ('P48 row commit', "            if row['commit'] != ref[0]:", "            if False:"),
 ('P52 manifest hash', "                if not m[2]:", "                if False:"),
 ('P58 ctl_manifest rc', "        any_failed = any_failed or not mbad\n        rc = max(rc, 0 if mbad else 1)", "        pass"),
 ('P60 watcher error', "        if err:\n            raise RigError(f'the watcher: {err[0]}')", "        pass"),
 ('P61 destroyed premise', "            if scen in ('destroy', 'gone') and kill == '' and not destroyed:", "            if False:"),
 ('P64 published.tsv lax', "if len(f) != 5 or not f[2].isdigit() or not f[3].isdigit():", "if len(f) != 5:"),
 ('P65 scenario unknown', "            if scen not in EXPECTED:\n                raise RigError", "            if False:\n                raise RigError"),
 ('P67 ctl_early b any', "b = next((n for n in order[1:] if pub[n]['txg'] > checked[a][0]['group']), None)", "b = next((n for n in order[1:]), None)"),
 ('P73 row none', "            if ref is None:\n                bad.append('no ref')", "            if False:\n                bad.append('no ref')"),
 ('P74 manifest absent', "            if m is None or m == 'absent' or data is None:", "            if data is None:"),
 ('Q01 EXPECTED both wrong', "'both': 'published',", "'both': 'abandoned destroyed',"),
 ('Q06 EXPECTED plain set', "'plain': None,", "'plain': 'absent',"),
 ('Q17 ctl_manifest verdict', "        put('ctl_manifest', 'CAUGHT' if mbad else 'MISSED',", "        put('ctl_manifest', 'CAUGHT',"),
 ('Q23 premise skips rest of case', "            if kill:\n                premise(f'premise_{cid}_killed', killed_at(kill, first_rc, perr, trace),\n                        f'killed at {kill}, as the trace and the hook\\'s line say')", "            if kill:\n                if not premise(f'premise_{cid}_killed', killed_at(kill, first_rc, perr, trace),\n                        f'killed at {kill}, as the trace and the hook\\'s line say'):\n                    continue"),
 ('R03 no judge_cut', "return bad + publish_check.judge_cut(cuts, row)", "return bad"),
 ('R06 any result', "and ln['result'] == f'{name} accepted'", "and ln['result'].startswith(name)"),
 ('R09 no row passes', "holds(f'pin_{cid}_carried_by_the_last_request', [f'no row of {t} checked at node B'], 'server', '')", "holds(f'pin_{cid}_carried_by_the_last_request', [], 'server', '')"),
 ('R11 first_carrier itself ignored', "if n > c and gr >= g and itself)", "if n > c and gr >= g)"),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
