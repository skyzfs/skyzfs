F = 'tests/kernel/agent_check.py'
T = 'python3 -I tests/kernel/test_agent_check.py'
# New plants at 1a40bfc: B1's reuse rule, B2's MOVED rule, D8, Codex's
# ordering, the empty trace; K08 and the fix probes adapted.
P = [
 ('K08a an empty or no-new-point trace is an error', "    if not trace or (not out and len(argv) == 1):", "    if not trace or not out:"),
 ('E01 an empty trace with a base is no error (old)', "    if not trace or (not out and len(argv) == 1):", "    if not out and len(argv) == 1:"),
 # B1
 ('N01 reuse: the commit not compared', "[] if later is not None and later[0] == reused else", "[] if later is not None else"),
 ('N02 reuse: T kept among the published', "(set(pub) | set(gone)) - ({t} if reused else set()),", "(set(pub) | set(gone)),"),
 ('N03 reuse also from the settle', "                reused = retry.get(t) if t else None", "                reused = (retry.get(t) or outs.get('settle', {}).get('published', {}).get(t)) if t else None"),
 ('N04 reuse never', "                reused = retry.get(t) if t else None", "                reused = None"),
 ('N05p probe: no reuse of an abandoned T', "                reused = retry.get(t) if t else None", "                reused = retry.get(t) if t and not any(t in o['abandoned'] for o in outs.values()) and t not in record_abandoned(text_of(os.path.join(cd, 'record-after.txt'))) else None"),
 ('N06 reuse: no INFO line', "                    put(f'obs_{cid}_name_reused', 'INFO', 'server',", "                    (lambda *a: None)(f'obs_{cid}_name_reused', 'INFO', 'server',"),
 # B2
 ('M01 MOVED without take:after', "MOVED = {('take', 'after'), ", "MOVED = {"),
 ('M02 MOVED without destroy:after (the engine takes T again there)', "         ('destroy', 'after'), ('manifest', 'before')", "         ('manifest', 'before')"),
 ('M03 MOVED with taken:after', "('taken', 'before')}", "('taken', 'before'), ('taken', 'after')}"),
 ('M04 absent for plain only', "    if moved(kill) and (scen == 'plain' or EXPECTED[scen] == 'published'):", "    if moved(kill) and scen == 'plain':"),
 ('M05 absent for every scenario', "    if moved(kill) and (scen == 'plain' or EXPECTED[scen] == 'published'):", "    if moved(kill):"),
 ('M06 intent:before:1 not set apart', "    if kill and (not t or kill == 'intent:before:1'):", "    if kill and not t:"),
 ('M07 intent:before at any reach', "    if kill and (not t or kill == 'intent:before:1'):", "    if kill and (not t or kill.startswith('intent:before:')):"),
 ('M08 moved: T itself counts', "                    new = sorted(n for n in outs.get('settle', {}).get('published', {}) if n != t)", "                    new = sorted(n for n in outs.get('settle', {}).get('published', {}))"),
 ('M09 moved never judged', "                if moved(kill) and t:", "                if False:"),
 ('M10 moved: the retry counts too', "                    new = sorted(n for n in outs.get('settle', {}).get('published', {}) if n != t)", "                    new = sorted(n for r in ('settle', 'retry') for n in outs.get(r, {}).get('published', {}) if n != t)"),
 ('M11 moved: any kill point', "    return bool(kill) and tuple(kill.split(':')[:2]) in MOVED", "    return bool(kill)"),
 # D8 and Codex's ordering
 ('D01 a case\'s own error not caught', "            except RigError as e:\n                # D8", "            except () as e:\n                # D8"),
 ('D02 a case\'s own error INFO only', "                premise(f'premise_{cid}_rig', [f'the check could not judge the case: {e}'], '')", "                put(f'premise_{cid}_rig', 'INFO', 'rig', str(e))"),
 ('X01 a bad verdict line raises', "                    premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')\n                    continue", "                    raise RigError(f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}')"),
 ('X02 a bad verdict line drops the rest', "                    premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')\n                    continue", "                    premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')\n                    break"),
]
PLANTS = [(pid, F, old, new, T) for pid, old, new in P]
