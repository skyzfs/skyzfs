# Set A at fafc771: the judge's survivors of #159's look at 11f02ef
# (n2_old_py.py), re-anchored where the code moved, and new plants on
# this delta's judge (G-a, G-b, G-c, K21, K22). plant2.py WORKTREE p3_A.py
TA = 'python3 -I tests/kernel/test_agent_check.py'
F = 'tests/kernel/agent_check.py'
PLANTS = [
 # The survivors of n2_old_py.py, as they were.
 ('P43 unmarked stray', F, "([] if HOLD_TAG in stray else ['the stray hold was released'])", '[]', TA),
 ('P46 criterion retry any', F, 'elif retry and all(n in pub or n in gone for n in retry):', 'elif retry:', TA),
 ('P48 row commit', F, "            if row['commit'] != ref[0]:", '            if False:', TA),
 ('P52 manifest hash', F, '                if not m[2]:', '                if False:', TA),
 ('P61 destroyed premise', F, "            if scen in ('destroy', 'gone') and kill == '' and not destroyed:", '            if False:', TA),
 ('P64 published.tsv lax', F, 'if len(f) != 5 or not f[2].isdigit() or not f[3].isdigit():', 'if len(f) != 5:', TA),
 ('P65 scenario unknown (re-indented)', F, '                if scen not in EXPECTED:\n                    raise RigError', '                if False:\n                    raise RigError', TA),
 ('P67 ctl_early b any', F, "b = next((n for n in order[1:] if pub[n]['txg'] > checked[a][0]['group']), None)", 'b = next((n for n in order[1:]), None)', TA),
 ('P73 row none', F, "            if ref is None:\n                bad.append('no ref')", "            if False:\n                bad.append('no ref')", TA),
 ('P74 manifest absent', F, "            if m is None or m == 'absent' or data is None:", '            if data is None:', TA),
 ('Q23 premise skips rest of case (re-indented)', F, "                if kill:\n                    premise(f'premise_{cid}_killed', killed_at(kill, first_rc, perr, trace),\n                            f'killed at {kill}, as the trace and the hook\\'s line say')", "                if kill:\n                    if not premise(f'premise_{cid}_killed', killed_at(kill, first_rc, perr, trace),\n                            f'killed at {kill}, as the trace and the hook\\'s line say'):\n                        continue", TA),
 ('N05 criterion: a gone retry fails', F, 'all(n in pub or n in gone for n in retry)', 'all(n in pub for n in retry)', TA),
 ('N07 gone keyed by case, last wins', F, '            gone[f[1]] = f[0]', '            gone = {f[1]: f[0]}', TA),
 ('K21 stays_absent for every T (re-indented)', F, "                if t and os.path.exists(os.path.join(rdir, srow)) \\\n                        and publish_check.read_snap_row(text_of(srow)).get('ref') is None:", '                if t and os.path.exists(os.path.join(rdir, srow)):', TA),
 ('K22 stays_absent: node B failure read as absent (re-indented)', F, "                    if rc_:\n                        raise RigError(f'skyzfs-nodeb snap-row {t}", "                    if False:\n                        raise RigError(f'skyzfs-nodeb snap-row {t}", TA),
 ('N05p probe: no reuse of an abandoned T', F, '                reused = retry.get(t) if t else None', "                reused = retry.get(t) if t and not any(t in o['abandoned'] for o in outs.values()) and t not in record_abandoned(text_of(os.path.join(cd, 'record-after.txt'))) else None", TA),
 # Q06 re-anchored: EXPECTED['plain'] is now 'published' and read by expected().
 ('Q06a EXPECTED plain absent', F, "    'plain': 'published',", "    'plain': 'absent',", TA),
 ('Q06b EXPECTED plain None', F, "    'plain': 'published',", "    'plain': None,", TA),
 # New, on this delta.
 ('Z01 given up: the record not read', F, "        or record_abandoned(text_of(os.path.join(cd, 'record-after.txt'))).get(t)", '        or None', TA),
 ('Z02 given up: the runs not read', F, "why = next((o['abandoned'][t] for o in outs.values() if t in o['abandoned']), None) \\", "why = None \\", TA),
 ('Z03 moved: any number of names', F, '([] if len(new) == 1 else', '([] if new else', TA),
 ('Z04 moved: reuse not excepted', F, ".split() and not reused", ".split()", TA),
 ('Z05 moved: the take by substring', F, "in text_of(os.path.join(cd, 'snapshots-after.txt')).split() and not reused", "in text_of(os.path.join(cd, 'snapshots-after.txt')) and not reused", TA),
 ('Z06 moved: the take not looked for', F, "                    left = f'{POOL}@{t}' in", "                    left = False and f'{POOL}@{t}' in", TA),
 ('Z07 given up: an INFO, not a pin', F, "                    holds(f'pin_{cid}_name_not_given_up',", "                    (lambda k, b, r, g: put(k, 'INFO', r, '; '.join(b) or g))(f'pin_{cid}_name_not_given_up',", TA),
 ('Z08 the course dir skipped', F, "        for cid in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:", "        for cid in [c for c in sorted(os.listdir(cdir)) if c != 'course'] if os.path.isdir(cdir) else []:", TA),
]
