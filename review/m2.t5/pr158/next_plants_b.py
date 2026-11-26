F = 'tests/kernel/agent_check.py'
T = 'python3 -I tests/kernel/test_agent_check.py'
# New plants on c580c72's fixes (D1, D2, D3, D4, G2, G3).
P = [
 # D1
 ('N01 gone pin never fails', "[f'{n} (case {c}) is published and {POOL}@{n} is not on node A' for n, c in gone.items()]", "[]"),
 ('N02 outcome ignores gone', "set(pub) | set(gone)", "set(pub)"),
 ('N03 gone line lax', "            if len(f) != 2 or not f[1]:\n                raise RigError(f'published-gone.tsv", "            if False:\n                raise RigError(f'published-gone.tsv"),
 ('N04 gone file optional', "text_of('published-gone.tsv')", "text_of('published-gone.tsv', need=False)"),
 ('N05 criterion: a gone retry fails', "all(n in pub or n in gone for n in retry)", "all(n in pub for n in retry)"),
 ('N06 gone pin is a premise (exit 2)', "        holds('pin_published_snapshots_exist',", "        (lambda k, b, r, g: premise(k, b, g))('pin_published_snapshots_exist',"),
 ('N07 gone keyed by case, last wins', "            gone[f[1]] = f[0]", "            gone = {f[1]: f[0]}"),
 # D2
 ('N10 first_run pin dropped', "                holds(f'pin_{cid}_first_run',", "                (lambda *a: None)(f'pin_{cid}_first_run',"),
 ('N11 first_run any status but 137', "[] if first_rc == FIRST_RC[scen]", "[] if first_rc != '137'"),
 ('N12 first_run only for a reach not made', "            elif scen != 'unmarked':", "            elif named and scen != 'unmarked':"),
 ('N13 first_run not for a reach not made', "            elif scen != 'unmarked':", "            elif not named and scen != 'unmarked':"),
 ('N14 FIRST_RC plain 1', "    'plain': '0',", "    'plain': '1',"),
 ('N15 FIRST_RC restart 7', "    'restart': '0',", "    'restart': '7',"),
 ('N16 FIRST_RC assist 1', "    'assist': '0',", "    'assist': '1',"),
 ('N17 FIRST_RC destroy 0', "    'destroy': '3',", "    'destroy': '0',"),
 ('N18 FIRST_RC tooold 0', "    'tooold': '3',", "    'tooold': '0',"),
 ('N19 FIRST_RC sweep 4', "    'sweep': '0',", "    'sweep': '4',"),
 ('N20 FIRST_RC gone 1', "    'gone': '0',", "    'gone': '1',"),
 ('N21 FIRST_RC dhook 7', "    'dhook': '0',", "    'dhook': '7',"),
 ('N22 FIRST_RC late 4', "    'late': '0',", "    'late': '4',"),
 ('N23 FIRST_RC both 1', "    'both': '0',", "    'both': '1',"),
 # D3
 ('N30 clean ignores the dump status', "    if record_rc.strip() != '0':", "    if False:"),
 ('N31 clean status passed as 0', "                        text_of(os.path.join(cd, 'record-after.rc')),", "                        '0',"),
 ('N32 clean: no status accepted', "    if record_rc.strip() != '0':", "    if record_rc.strip() not in ('0', ''):"),
 ('N33 unmarked record-before.rc ignored', "\n                         and text_of(os.path.join(cd, 'record-before.rc')).strip() == '0'", ""),
 ('N34 clean status file optional', "                        text_of(os.path.join(cd, 'record-after.rc')),", "                        text_of(os.path.join(cd, 'record-after.rc'), need=False) or '0',"),
 # D4
 ('N40 sweep first publish never fails', "holds(f'pin_{cid}_first_publish', [] if frc == '0' else", "holds(f'pin_{cid}_first_publish', [] if True else"),
 ('N41 sweep first.rc optional', "frc = text_of(os.path.join(cd, 'first.rc')).strip()", "frc = text_of(os.path.join(cd, 'first.rc'), need=False).strip() or '0'"),
 # G2
 ('N50 both pin never fails', "holds(f'pin_{cid}_daemon_killed', [] if krc == '137' else", "holds(f'pin_{cid}_daemon_killed', [] if True else"),
 ('N51 both any non-zero', "[] if krc == '137' else", "[] if krc != '0' else"),
 ('N52 both: no status accepted', "[] if krc == '137' else", "[] if krc in ('137', '') else"),
 # G3
 ('N60 final override removed', "    if any_failed:\n        rc = 1\n", ""),
 ('N61 holds does not mark failed', "            any_failed = True\n            put(key, 'FAILED'", "            put(key, 'FAILED'"),
 ('N62 ctl_early not marked', "        any_failed = any_failed or not ok\n        rc = max(rc, 0 if ok else 1)\n        wrong = dict", "        rc = max(rc, 0 if ok else 1)\n        wrong = dict"),
 ('N63 ctl_files not marked', "        any_failed = any_failed or not ok\n        rc = max(rc, 0 if ok else 1)\n        mbad = judge", "        rc = max(rc, 0 if ok else 1)\n        mbad = judge"),
 ('N64 ctl_manifest not marked', "        any_failed = any_failed or not mbad\n", ""),
 ('N65 override is max (old rule)', "    if any_failed:\n        rc = 1\n", "    if any_failed:\n        rc = max(rc, 1)\n"),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
