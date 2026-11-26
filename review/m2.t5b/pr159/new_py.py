F = 'tests/kernel/agent_check.py'
T = 'python3 -I tests/kernel/test_agent_check.py'
# Set B's plants that no longer apply, adapted to this head, then new
# plants on this head's judge (G1, D5, G6, G7, G9).
P = [
 ('N12a first_run only for a reach not made', "            elif scen in FIRST_RC:", "            elif named and scen in FIRST_RC:"),
 ('N13a first_run not for a reach not made', "            elif scen in FIRST_RC:", "            elif not named and scen in FIRST_RC:"),
 ('S13a new_points first reach only', "NEW_REACHES = 3", "NEW_REACHES = 1"),
 # G1
 ('K01 new_points from the start', "first = next((i for i, p in enumerate(pts) if p not in had), len(pts))", "first = 0"),
 ('K02 new_points set difference (old)', "    for p in pts[first:]:\n        e, s, n = p.split(':')\n        if int(n)", "    for p in pts[first:]:\n        if p in had:\n            continue\n        e, s, n = p.split(':')\n        if int(n)"),
 ('K03 new_points no last reach', "if int(n) <= NEW_REACHES or int(n) == last[(e, s)]:", "if int(n) <= NEW_REACHES:"),
 ('K04 NEW_REACHES 2', "NEW_REACHES = 3", "NEW_REACHES = 2"),
 ('K05 NEW_REACHES 4', "NEW_REACHES = 3", "NEW_REACHES = 4"),
 ('K06 last is the first reach', "        last[(e, s)] = int(n)", "        last.setdefault((e, s), int(n))"),
 ('K07 skip the first new point', "    for p in pts[first:]:", "    for p in pts[first + 1:]:"),
 ('K08 an empty rare list is an error', "    if not out and len(argv) == 1:", "    if not out:"),
 ('K09 last over the new part only', "    for p in pts:\n        e, s, n = p.split(':')\n        last[(e, s)] = int(n)", "    for p in pts[:first]:\n        e, s, n = p.split(':')\n        last[(e, s)] = int(n)\n    for p in pts:\n        e, s, n = p.split(':')\n        last.setdefault((e, s), int(n))"),
 # G6 / D5 in the judge
 ('K10 rig-error.tsv no exit 2', "            c, _, m = ln.partition('\\t')\n            rc = max(rc, 2)\n", "            c, _, m = ln.partition('\\t')\n"),
 ('K11 rig-error.tsv ignored', "        for ln in text_of('rig-error.tsv', need=False).splitlines():", "        for ln in []:"),
 ('K12 points-error INFO', "            premise(f'premise_{c}_points', [f'no points from the trace of case {c}: {m}'], '')", "            put(f'premise_{c}_points', 'INFO', 'rig', m)"),
 ('K13 verdicts only for listed cases', "        for cid in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:", "        for cid in sorted(ln.split('\\t')[0] for ln in text_of('cases.tsv').splitlines() if ln):"),
 ('K14 verdicts as premises', "                holds(f'pin_{cid}_{pin}', [what], 'server', '')", "                premise(f'pin_{cid}_{pin}', [what], '')"),
 ('K15 verdict name not checked', "                if not re.fullmatch(r'[a-z][a-z_]*', pin) or not what:", "                if not what:"),
 ('K16 case rig error silent', "                premise(f'premise_{cid}_rig', [f\"the rig's own error in the case: {cerr}\"], '')\n", ""),
 ('K17 case rig error judged further', "                put(f'case_{cid}', 'INFO', 'rig', f'{scen}{\" killed at \" + kill if kill else \"\"}: not judged')\n                continue\n", "                put(f'case_{cid}', 'INFO', 'rig', f'{scen}{\" killed at \" + kill if kill else \"\"}: not judged')\n"),
 ('K18 gone/both with no kill not an error', "            elif scen in KILLED_ONLY:\n                premise(", "            elif scen in KILLED_ONLY and False:\n                premise("),
 ('K19 gone/both with no kill judged on FIRST_RC 0', "            elif scen in KILLED_ONLY:", "            elif scen in ():"),
 # G7
 ('K20 stays_absent never fails', "                holds(f'pin_{cid}_stays_absent', [] if later is None else", "                holds(f'pin_{cid}_stays_absent', [] if True else"),
 ('K21 stays_absent for every T', "            if t and os.path.exists(os.path.join(rdir, srow)) \\\n                    and publish_check.read_snap_row(text_of(srow)).get('ref') is None:", "            if t and os.path.exists(os.path.join(rdir, srow)):"),
 ('K22 stays_absent: node B failure read as absent', "                if rc_:\n                    raise RigError(f'skyzfs-nodeb snap-row {t}", "                if False:\n                    raise RigError(f'skyzfs-nodeb snap-row {t}"),
 ('K23 stays_absent only without snaprow file', "            if t and os.path.exists(os.path.join(rdir, srow)) \\", "            if t and not os.path.exists(os.path.join(rdir, srow)) \\"),
 ('K24 stays_absent skipped when the retry printed T', "            if t and os.path.exists(os.path.join(rdir, srow)) \\", "            if t and t not in retry and os.path.exists(os.path.join(rdir, srow)) \\"),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
