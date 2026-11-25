#!/usr/bin/env python3
"""Plants on the publish rig's check, agent and course script at c7cc03f
(the next look at #139, M2.T7).

Each plant replaces text that must occur exactly once in its file (else
NOT APPLIED), runs the pull request's own tests of that file (each under
a 600 s limit), and puts the file back in a finally. KILLED: a test
failed (or timed out); SURVIVED: every test passed.

The suites: publish_check.py -> test_publish_check.py and
test_publish_check_main.py (the new driver of main); publish_agent.py ->
test_publish_agent.py; cut-record.sh -> test_cut_record.sh.

Usage: next_plants_py.py WORKTREE OUT.json [ID...]
"""
import json
import os
import shutil
import subprocess
import sys

PC = 'tests/kernel/publish_check.py'
PA = 'tests/kernel/publish_agent.py'
SH = 'tests/kernel/cut-record.sh'
SUITES = {
    PC: [['python3', 'tests/kernel/test_publish_check.py'], ['python3', 'tests/kernel/test_publish_check_main.py']],
    PA: [['python3', 'tests/kernel/test_publish_agent.py']],
    SH: [['bash', 'tests/kernel/test_cut_record.sh']],
}

PLANTS = [
    # B4: the tables, loosened (E1 to E17 as at d169139; E17 to E20 new)
    ('E1', PC, 't1-other-end may end done', [("'t1-other-end': ('refused taken',),", "'t1-other-end': ('refused taken', 'done '),")]),
    ('E2', PC, 'tw-after may end done', [("'tw-after': ('withdrawn', 'unknown'),", "'tw-after': ('withdrawn', 'unknown', 'done '),")]),
    ('E3', PC, 'tw-last may end done', [("'tw-last': ('withdrawn', 'unknown'),", "'tw-last': ('withdrawn', 'unknown', 'done '),")]),
    ('E4', PC, 'tk-after may read pending', [("'tk-after': ('unknown',),", "'tk-after': ('unknown', 'pending'),")]),
    ('E5', PC, 'att: any refusal', [("'att': ('refused attachment',),", "'att': ('refused',),")]),
    ('E6', PC, 't5 may end done', [("'t5': ('expired',),", "'t5': ('expired', 'done '),")]),
    ('E7', PC, 'peer: any refusal', [("'peer': ('refused peer',),", "'peer': ('refused',),")]),
    ('E8', PC, 'm1-after may be done', [("'m1-after': ('unknown',),", "'m1-after': ('unknown', 'done '),")]),
    ('E9', PC, 't7-withdraw may still pend', [("'t7-withdraw': ('withdrawn',),", "'t7-withdraw': ('withdrawn', 'pending'),")]),
    ('E10', PC, 't7-other accepted', [("'t7-other': ('refused asked',),", "'t7-other': ('refused asked', 'accepted'),")]),
    ('E11', PC, 'nu may answer cuts', [("'nu': ('refused no-upload',),", "'nu': ('refused no-upload', 'cuts '),")]),
    ('E12', PC, 't1-again-end may be refused', [("'t1-again-end': ('done ',),", "'t1-again-end': ('done ', 'refused'),")]),
    ('E13', PC, 'ABSENT drops tw', [("ABSENT = {'pplain': ('t5', 't3'), 'pkill': ('tw',)}", "ABSENT = {'pplain': ('t5', 't3'), 'pkill': ()}")]),
    ('E14', PC, 'ABSENT drops t5', [("ABSENT = {'pplain': ('t5', 't3'), 'pkill': ('tw',)}", "ABSENT = {'pplain': ('t3',), 'pkill': ('tw',)}")]),
    ('E15', PC, 'PUBLISHED drops t4', [("'pplain': {'t1': 't1', 't2': 't2', 't4': 't4', 't7': 't7-end'},", "'pplain': {'t1': 't1', 't2': 't2', 't7': 't7-end'},")]),
    ('E16', PC, 'BETWEEN: m1 judged as a premise that it landed', [("BETWEEN = {'pkillman': ('m1', False),", "BETWEEN = {'pkillman': ('m1', True),")]),
    ('E17', PC, 'COUNTS: control_peer 0', [("'control_peer': 1, 'control_malformed': 2}}", "'control_peer': 0, 'control_malformed': 2}}")]),
    ('E18', PC, 'COUNTS: control_answered not pinned', [("COUNTS = {'pchannel': {'control_answered': 4, ", "COUNTS = {'pchannel': {")]),
    ('E19', PC, 'TW_STATUS: publish_pending not pinned', [("TW_STATUS = {'publish_withdrawn': 1, 'publish_pending': 0, 'publish_published': 0}", "TW_STATUS = {'publish_withdrawn': 1, 'publish_published': 0}")]),
    ('E20', PC, "KILLED_BY: pkillpub's words those of any kill", [("             'pkillpub': 'after the transaction of publish 1 (p1) landed'}", "             'pkillpub': 'killed'}")]),
    # N1/M3: main's wiring (M1 to M14 as at d169139, M8 and M9 on the new premises)
    ('M1', PC, 'premise t4 always holds', [("        premise('premise_t4_cuts_below_g', bool(below),", "        premise('premise_t4_cuts_below_g', True,")]),
    ('M2', PC, "ctl_below put at the row's own group (cannot fail)", [("dict(req, group=row['group'] + 1)", "dict(req, group=row['group'])")]),
    ('M3', PC, 'a MISSED row control does not fail the run', [("                    rc = max(rc, 0 if bad_ else 1)", "                    rc = max(rc, 0)")]),
    ('M3f', PC, 'a MISSED ctl_files does not fail the run', [("                rc = max(rc, 0 if ok else 1)", "                rc = max(rc, 0)")]),
    ('M4', PC, 'ctl_files run with the right digests', [("                cseen = nb.verify(vol, f'snap/{t}', t, wrong, damaged=False)", "                cseen = nb.verify(vol, f'snap/{t}', t, want, damaged=False)")]),
    ('M5', PC, 'ABSENT judged by the ref alone', [("                                                          ('a row', got['row'] is not None)) if there],", "                                                          ('a row', False)) if there],")]),
    ('M6', PC, 'pin_m1 inverted', [("'transaction'] if is_there else [], 'server',", "'transaction'] if not is_there else [], 'server',")]),
    ('M7', PC, 'premise prewind held: any status', [("            premise('premise_held_at_rewind', held.isdigit() and int(held) > 0,", "            premise('premise_held_at_rewind', True,")]),
    ('M8', PC, 'premise_rewind_cut_committed always holds', [("            premise('premise_rewind_cut_committed', not bad,", "            premise('premise_rewind_cut_committed', True,")]),
    ('M9', PC, 'premise_tw_committed_past_g always holds', [("            premise('premise_tw_committed_past_g', committed,", "            premise('premise_tw_committed_past_g', True,")]),
    ('M10', PC, 'asking twice compared with the first done itself', [("                again = done_commit(ends['t1-again-end'])", "                again = commit")]),
    ('M11', PC, 'node B stages not judged', [("            holds(f'pin_{t}_nodeb', [f'node B failed at {failed}: {seen.text()}'] if failed else [],", "            holds(f'pin_{t}_nodeb', [],")]),
    ('M12', PC, 'pin_T_cut not judged', [("                holds(f'pin_{t}_cut', judge_cut(cuts, row), 'server',", "                holds(f'pin_{t}_cut', [], 'server',")]),
    ('M13', PC, 'the controls run on no name', [("            if name == 'pplain' and not controlled and isinstance(row, dict):", "            if False:")]),
    ('M14', PC, 'ctl_files: any catch counts', [("                verdict, ok, _ = nodeb_check.control_verdict(cseen, ('files',))", "                verdict, ok, _ = nodeb_check.control_verdict(cseen, nodeb_check.STAGES)")]),
    # B2: prewind's premise
    ('P1', PC, 'rewind_committed: a cut that opens as itself passes', [("    elif any(c[2] for c in same):", "    elif False:")]),
    ('P2', PC, 'rewind_committed: r1 made before the cut passes', [("    if named is not None and named < number:", "    if False:")]),
    ('P3', PC, 'rewind_committed: r1 made BY the cut is the rig error (hides the verdict)', [("    if named is not None and named < number:", "    if named is not None and named <= number:")]),
    ('P4', PC, 'rewind_committed: C not checked', [("    if number <= req['after']:", "    if False:")]),
    ('P5', PC, 'rewind_committed: G not checked', [("    if group < req['group']:\n        bad.append(f\"the row's cut is of group {group}", "    if False:\n        bad.append(f\"the row's cut is of group {group}")]),
    ('P6', PC, 'rewind_committed: state not checked', [("    if state != req['state']:", "    if False:")]),
    ('P7', PC, 'rewind_committed: a cut with no line passes', [("    if not same:\n        bad.append(f'no cut line of number", "    if False:\n        bad.append(f'no cut line of number")]),
    ('P8', PC, 'rewind_committed: the cut matched by number alone', [("    same = [c for c in cuts if c[0] == number and c[1] == group]", "    same = [c for c in cuts if c[0] == number]")]),
    ('P9', PC, 'prewind: named read as the row cut (premise never sees r1)', [("            named = got['row']['number'] if isinstance(got.get('row'), dict) else None", "            named = None")]),
    # B3: pkill's premise
    ('Q1', PC, "withdrawn_past: every instance's cuts after the line (B3 as it was)", [("                return inst['pid'], [(int(c['sectors']), int(c['result'])) for c in inst['lines']", "                return inst['pid'], [(int(c['sectors']), int(c['result'])) for i2 in instances for c in i2['lines']")]),
    ('Q2', PC, 'withdrawn_past: cuts before the withdrawal count', [("if c['event'] == 'cut' and c['line'] > ln['line'] and c['sector'] == 'itself'", "if c['event'] == 'cut' and c['sector'] == 'itself'")]),
    ('Q3', PC, 'withdrawn_past: rewind cuts count', [("if c['event'] == 'cut' and c['line'] > ln['line'] and c['sector'] == 'itself'", "if c['event'] == 'cut' and c['line'] > ln['line']")]),
    ('Q4', PC, 'withdrawn_past: G not checked', [("and c['result'].isdigit() and int(c['result']) >= group]", "and c['result'].isdigit()]")]),
    ('Q5', PC, 'committed: any cut past G counts, not the committed one', [("(row[1], row[2]) in past\n", "bool(past)\n")]),
    ('Q6', PC, 'committed: state not checked', [("committed = row is not None and row[0] == tw['state'] and row[1] > tw['after'] and", "committed = row is not None and row[1] > tw['after'] and")]),
    ('Q7', PC, 'committed: C not checked', [("row[0] == tw['state'] and row[1] > tw['after'] and", "row[0] == tw['state'] and")]),
    ('Q8', PC, 'pin_tw_withdrawn_status not judged', [("            holds('pin_tw_withdrawn_status', exact_counts(status_of(text_of('tw-status.txt')), TW_STATUS),", "            holds('pin_tw_withdrawn_status', [],")]),
    ('Q9', PC, "withdrawn_past: any answer to tw's withdrawal", [("ln['result'] == f'{name} withdrawn':", "ln['result'].startswith(f'{name} '):")]),
    ('Q10', PC, 'withdrawn_past: any op', [("if ln['event'] == 'publish' and ln['op'] == 'withdraw' and", "if ln['event'] == 'publish' and")]),
    # N3: the kill's premise and pin_p1_found
    ('K1', PC, 'killed_by: kill.tsv not read', [("kill == 'killed_rc\\t137' and KILLED_BY[name] in serve,", "KILLED_BY[name] in serve,")]),
    ('K2', PC, "killed_by: serve.log's words not read", [("kill == 'killed_rc\\t137' and KILLED_BY[name] in serve,", "kill == 'killed_rc\\t137',")]),
    ('K3', PC, 'pin_p1_found not judged', [("holds(f'pin_{t}_found', [] if got['ref'] is not None and got['ref'][0] == end else", "holds(f'pin_{t}_found', [] if True else")]),
    ('K4', PC, 'pin_p1_found: any ref after the kill', [("[] if got['ref'] is not None and got['ref'][0] == end else", "[] if got['ref'] is not None else")]),
    # N6: the channel's counts and the socket's mode
    ('C1', PC, 'counts judged at least', [("if status.get(k) != str(v)]", "if not (status.get(k) or '').isdigit() or int(status.get(k)) < v]")]),
    ('C2', PC, 'socket mode: any', [("[] if mode == '600' else", "[] if True else")]),
    ('C3', PC, 'socket mode: 0666 passes', [("[] if mode == '600' else", "[] if mode in ('600', '666') else")]),
    ('C4', PC, 'exact_counts: a key missing passes', [("if status.get(k) != str(v)]", "if status.get(k, str(v)) != str(v)]")]),
    # N5: the raw client and the answer
    ('A13', PA, 'answer_of: an unended line is the answer', [("    whole = text.split('\\n')[:-1]", "    whole = text.split('\\n')")]),
    ('A14', PA, 'answer_of: the greeting alone is the answer', [("    rest = whole[1:] if whole and whole[0] == HELLO else whole", "    rest = whole")]),
    ('A15', PA, "raw: nothing written is the rig's error again", [("            print(f'raw {label}: the daemon closed with nothing written: {err}', file=sys.stderr)", "            raise err")]),
    ('A16', PA, "raw: EPIPE on the write is the rig's error (X1)", [("        except (BrokenPipeError, ConnectionResetError) as e:\n            err = e", "        except ConnectionResetError as e:\n            err = e")]),
    ('A17', PA, 'raw: a reset on the read raises (X4)', [("            except ConnectionResetError as e:\n                err = err or e\n                break", "            except ConnectionResetError as e:\n                raise")]),
    ('A18', PA, "answer_of: bytes with no whole line are ''", [("    return 'no answer line: ' +", "    return '' if True else 'no answer line: ' +")]),
    ('A19', PA, "answer_of: no whole line after the greeting: the greeting", [("    if rest:\n        return rest[0]", "    if whole:\n        return rest[0] if rest else whole[0]")]),
    ('A20', PA, 'raw: the exit for a refusal is 1 again', [("            print(raw(argv[2], argv[3], argv[4] if len(argv) > 4 else 'raw', sys.stdin.buffer.read()))\n            return 0", "            a = raw(argv[2], argv[3], argv[4] if len(argv) > 4 else 'raw', sys.stdin.buffer.read())\n            print(a)\n            return 1 if a.startswith('refused ') else 0")]),
    # The course script (B2, B3, N6)
    ('S1', SH, 'prewind: the agent syncs groups again', [("    pflow r1 r1 --limit-s 900 &", "    pflow r1 r1 --sync --limit-s 900 &")]),
    ('S2', SH, 'prewind: the store back before the answer', [("    asked r1 120 || rig_error \"the agent's publish of r1 had no answer 120 s on\"\n    bash -c \"$S3_UP\" > \"$RDIR/s3-up.log\" 2>&1 || rig_error \"CUT_S3_UP failed: $(tail -n 2 \"$RDIR/s3-up.log\")\"", "    bash -c \"$S3_UP\" > \"$RDIR/s3-up.log\" 2>&1 || rig_error \"CUT_S3_UP failed: $(tail -n 2 \"$RDIR/s3-up.log\")\"\n    asked r1 120 || rig_error \"the agent's publish of r1 had no answer 120 s on\"")]),
    ('S3', SH, 'prewind: row_past nudges', [("    row_past rewind-row.txt \"$(preq r1 5)\" \"$(preq r1 2)\" 180 || true", "    row_past rewind-row.txt \"$(preq r1 5)\" \"$(preq r1 2)\" 180 nudge || true")]),
    ('S4', SH, 'pkill: row_past does not nudge', [("    row_past tw-row.txt \"$(preq tw 5)\" \"$(preq tw 2)\" 300 nudge || true", "    row_past tw-row.txt \"$(preq tw 5)\" \"$(preq tw 2)\" 300 || true")]),
    ('S5', SH, 'pkill: no wait for the commit past G', [("    row_past tw-row.txt \"$(preq tw 5)\" \"$(preq tw 2)\" 300 nudge || true\n", "    nbrow tw-row.txt\n")]),
    ('S6', SH, 'pchannel: the mode recorded after the rig widened it', [("    stat -c %a \"$sock\" > \"$RDIR/socket-mode.txt\"\n    chmod 0666 \"$sock\"", "    chmod 0666 \"$sock\"\n    stat -c %a \"$sock\" > \"$RDIR/socket-mode.txt\"")]),
    ('S7', SH, 'row_past: a cut at C passes', [("[ \"$n\" -gt \"$2\" ] && [ \"$g\" -ge \"$3\" ] && return 0", "[ \"$n\" -ge \"$2\" ] && [ \"$g\" -ge \"$3\" ] && return 0")]),
    ('S8', SH, 'row_past: G not checked', [("[ \"$n\" -gt \"$2\" ] && [ \"$g\" -ge \"$3\" ] && return 0", "[ \"$n\" -gt \"$2\" ] && return 0")]),
    ('S9', SH, 'asked: a line with no answer passes', [("$3 ~ (\"^publish --name \" t \" \") && $5 != \"\" { f = 1 }", "$3 ~ (\"^publish --name \" t \" \") { f = 1 }")]),
    ('S10', SH, 'prewind: row_past with the arguments swapped (C and G)', [("    row_past rewind-row.txt \"$(preq r1 5)\" \"$(preq r1 2)\" 180 || true", "    row_past rewind-row.txt \"$(preq r1 2)\" \"$(preq r1 5)\" 180 || true")]),
    ('S11', SH, 'pkill: the status read before the commit past G', [("    row_past tw-row.txt \"$(preq tw 5)\" \"$(preq tw 2)\" 300 nudge || true\n    ustatus tw-status.txt", "    ustatus tw-status.txt\n    row_past tw-row.txt \"$(preq tw 5)\" \"$(preq tw 2)\" 300 nudge || true")]),
    ('S12', SH, 'pchannel: status_until dropped (B1 back)', [("    status_until channel-status.txt control_peer 1 10 || true", "    ustatus channel-status.txt")]),
]


def run(cmd, cwd):
    # No bytecode is read or written, so that a plant never runs an
    # earlier plant's cached code.
    shutil.rmtree(os.path.join(cwd, 'tests/kernel/__pycache__'), ignore_errors=True)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=600, env=env)
        return p.returncode, (p.stdout + p.stderr)[-3000:]
    except subprocess.TimeoutExpired:
        return 'timeout', ''


def suites(wt, fn):
    bad = []
    for cmd in SUITES[fn]:
        rc, out = run(cmd, wt)
        if rc != 0:
            what = [ln for ln in out.splitlines() if ln.startswith(('FAIL:', 'ERROR:', 'not ok', 'FAIL '))]
            bad.append(f'{os.path.basename(cmd[-1])}: rc {rc}: ' + ' | '.join(what)[:300])
    return bad


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    base = {fn: suites(wt, fn) for fn in SUITES}
    print('baseline', base, flush=True)
    if any(base.values()):
        sys.exit(2)
    results = []
    for pid, fn, what, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, fn)
        with open(path) as f:
            orig = f.read()
        mode = os.stat(path).st_mode
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:60]!r})'
                break
            text = text.replace(old, new)
        res = {'id': pid, 'file': fn, 'what': what}
        if status is None:
            try:
                with open(path, 'w') as f:
                    f.write(text)
                bad = suites(wt, fn)
            finally:
                with open(path, 'w') as f:
                    f.write(orig)
                os.chmod(path, mode)
            res.update(result='KILLED' if bad else 'SURVIVED', failed=bad)
        else:
            res.update(result=status)
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        res['clean_after'] = clean.strip() == ''
        results.append(res)
        print(f"{pid:4} {res['result']:9} {what}", flush=True)
        with open(outp, 'w') as f:
            json.dump(results, f, indent=1)


if __name__ == '__main__':
    main()
