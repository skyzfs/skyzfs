#!/usr/bin/env python3
"""Plants on the channel rig's judge and agent, and on the withheld pick
(the review of #139, M2.T7, at cabd5a0).

Each plant replaces text that must occur exactly once in its file (else
NOT APPLIED), runs the pull request's unit tests of that file's area
(each under a 600 s limit), then the review's driver of
publish_check.main() (drive_publish_check.py), and puts the file back in
a finally. KILLED: a test failed; SURVIVED: every test passed. A plant
is reported for the PR's tests and for the driver separately.
Usage: plants_py.py WORKTREE OUT.json [ID...]
"""
import json
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PC = 'tests/kernel/publish_check.py'
PA = 'tests/kernel/publish_agent.py'
CC = 'tests/kernel/cut_check.py'
NC = 'tests/kernel/nodeb_check.py'
SUITES = {
    PC: ['test_publish_check.py'],
    PA: ['test_publish_agent.py'],
    CC: ['test_cut_check.py', 'test_nodeb_check.py'],
    NC: ['test_nodeb_check.py'],
}

PLANTS = [
    # EXPECTED, flipped or loosened
    ('E1', PC, "t1-other-end may end done", [("'t1-other-end': ('refused taken',),", "'t1-other-end': ('refused taken', 'done '),")]),
    ('E2', PC, 'tw-after may end done', [("'tw-after': ('withdrawn', 'unknown'),", "'tw-after': ('withdrawn', 'unknown', 'done '),")]),
    ('E3', PC, 'tw-last may end done', [("'tw-last': ('withdrawn', 'unknown'),", "'tw-last': ('withdrawn', 'unknown', 'done '),")]),
    ('E4', PC, 'tk-after may read pending (the next instance knew it)', [("'tk-after': ('unknown',),", "'tk-after': ('unknown', 'pending'),")]),
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
    ('E17', PC, 'COUNTS: peer at least 0', [("COUNTS = {'pchannel': {'control_peer': 1,", "COUNTS = {'pchannel': {'control_peer': 0,")]),
    # The judge
    ('J1', PC, 'judge_row: no check of G', [("    if row['group'] < req['group']:", "    if False and row['group'] < req['group']:")]),
    ('J2', PC, 'judge_row: a cut counted at C passes', [("    if row['number'] <= req['after']:", "    if row['number'] < req['after']:")]),
    ('J3', PC, 'judge_row: no check of the state', [("    if row['state'] != req['state']:", "    if False:")]),
    ('J4', PC, "judge_row: no check of the ref against done", [("    elif ref[0] != commit:", "    elif False:")]),
    ('J5', PC, "judge_row: no check of the row's commit against the ref", [("    if ref is not None and row['commit'] != ref[0]:", "    if False:")]),
    ('J6', PC, 'judge_row: no ref passes', [("        bad.append('no ref snap/T')", "        pass")]),
    ('J7', PC, 'judge_manifest: the bytes sent not compared', [("    if got.get('fetched') is not None and got['fetched'] != sent:", "    if False:")]),
    ('J8', PC, "judge_manifest: node B's matches not read", [("    if not m[2]:", "    if False:")]),
    ('J9', PC, 'judge_cut: rewind passes', [("    if not any(c[1] == row['group'] and c[2] for c in same):", "    if False:")]),
    ('J10', PC, "judge_cut: the cut's group not compared", [("    if not any(c[1] == row['group'] for c in same):", "    if False:")]),
    ('J11', PC, 'counted_between: the upper bound inclusive', [("    return [c for c in cuts if c[0] > after and lo <= c[1] < hi]", "    return [c for c in cuts if c[0] > after and lo <= c[1] <= hi]")]),
    ('J12', PC, 'counted_between: C ignored', [("    return [c for c in cuts if c[0] > after and lo <= c[1] < hi]", "    return [c for c in cuts if lo <= c[1] < hi]")]),
    ('J13', PC, 'judge_ends: a prefix anywhere in the answer', [("any(ends[k].startswith(a) for a in want[k])", "any(a in ends[k] for a in want[k])")]),
    ('J14', PC, 'judge_ends: a missing label is skipped', [("    if missing:\n        raise RigError(f'the course left no answer", "    if False:\n        raise RigError(f'the course left no answer")]),
    ('J15', PC, 'done_commit: any done', [("    if len(f) == 2 and f[0] == 'done' and f[1].startswith('commit='):", "    if f and f[0] == 'done' and len(f) > 1:")]),
    ('J16', PC, 'read_snap_row: ref none read as there', [("            got['ref'] = None if f[1:] == ['none'] else", "            got['ref'] = ('none', '0') if f[1:] == ['none'] else")]),
    # main's wiring
    ('M1', PC, 'premise t4 always holds', [("        premise('premise_t4_cuts_below_g', bool(below),", "        premise('premise_t4_cuts_below_g', True,")]),
    ('M2', PC, 'ctl_below put at the row\'s own group (cannot fail)', [("dict(req, group=row['group'] + 1)", "dict(req, group=row['group'])")]),
    ('M3', PC, 'a MISSED control does not fail the run', [("                    rc = max(rc, 0 if bad_ else 1)", "                    rc = max(rc, 0)")]),
    ('M4', PC, 'ctl_files run with the right digests', [("                cseen = nb.verify(vol, f'snap/{t}', t, wrong, damaged=False)", "                cseen = nb.verify(vol, f'snap/{t}', t, want, damaged=False)")]),
    ('M5', PC, 'ABSENT judged by the ref alone', [("                                                          ('a row', got['row'] is not None)) if there],", "                                                          ('a row', False)) if there],")]),
    ('M6', PC, 'pin_m1 inverted', [("'transaction'] if is_there else [], 'server',", "'transaction'] if not is_there else [], 'server',")]),
    ('M7', PC, 'premise prewind held: any status', [("            premise('premise_held_at_rewind', held.isdigit() and int(held) > 0,", "            premise('premise_held_at_rewind', True,")]),
    ('M8', PC, 'premise abandoned: none needed', [("            premise('premise_abandoned_cuts_at_g', bool(old),", "            premise('premise_abandoned_cuts_at_g', True,")]),
    ('M9', PC, 'premise tw past G: none needed', [("            premise('premise_tw_cuts_past_g', bool(past),", "            premise('premise_tw_cuts_past_g', True,")]),
    ('M10', PC, 'asking twice compared with the first done itself', [("                again = done_commit(ends['t1-again-end'])", "                again = commit")]),
    ('M11', PC, 'node B stages not judged', [("            holds(f'pin_{t}_nodeb', [f'node B failed at {failed}: {seen.text()}'] if failed else [],", "            holds(f'pin_{t}_nodeb', [],")]),
    ('M12', PC, 'pin_T_cut not judged', [("                holds(f'pin_{t}_cut', judge_cut(cuts, row), 'server',", "                holds(f'pin_{t}_cut', [], 'server',")]),
    ('M13', PC, 'the controls run on no name', [("            if name == 'pplain' and not controlled and isinstance(row, dict):", "            if False:")]),
    ('M14', PC, 'ctl_files: any catch counts', [("                verdict, ok, _ = nodeb_check.control_verdict(cseen, ('files',))", "                verdict, ok, _ = nodeb_check.control_verdict(cseen, nodeb_check.STAGES)")]),
    # The agent
    ('A1', PA, 'cuts read after the snapshot (C after pool@T)', [("    verb, cuts, code, text = agent.cuts()\n    if verb == 'refused':\n        return f'refused {code} {text}'\n    sh_(['zfs', 'snapshot', f'{pool}@{name}'])", "    sh_(['zfs', 'snapshot', f'{pool}@{name}'])\n    verb, cuts, code, text = agent.cuts()\n    if verb == 'refused':\n        return f'refused {code} {text}'")]),
    ('A2', PA, 'the hold placed after the publish', [("    if hold:\n        sh_(['zfs', 'hold', 'skyzfs-publish', f'{pool}@{name}'])\n    try:", "    try:"), ("        t0 = now()\n        while True:", "        if hold:\n            sh_(['zfs', 'hold', 'skyzfs-publish', f'{pool}@{name}'])\n        t0 = now()\n        while True:")]),
    ('A3', PA, 'G+K ignored', [("        group = int(txg) + group_plus", "        group = int(txg)")]),
    ('A4', PA, 'published.tsv records createtxg as G', [("        f.write(f'{name}\\t{group}\\t{cuts[\"attachment\"]}", "        f.write(f'{name}\\t{txg}\\t{cuts[\"attachment\"]}")]),
    ('A5', PA, '--twice asks once', [("        for _ in range(2 if twice else 1):", "        for _ in range(1):")]),
    ('A6', PA, 'sync without a write', [("                (nudge or write_nudge)(os.path.join(mnt, 'publish-nudge'))\n", "")]),
    ('A7', PA, 'the hold released only on success', [("    finally:\n        if hold:\n            sh_(['zfs', 'release'", "    except Exception:\n        raise\n    else:\n        if hold:\n            sh_(['zfs', 'release'")]),
    ('A8', PA, 'raw: the greeting taken as the answer', [("    answer = lines[1] if len(lines) > 1 and lines[0] == HELLO else (lines[0] if lines else '')", "    answer = lines[0] if lines else ''")]),
    ('A9', PA, 'a done commit of any length', [("    if verb == 'done' and not is_hex(fields['commit'], 64):", "    if verb == 'done' and not fields['commit']:")]),
    ('A10', PA, 'a refusal with exit 0 accepted', [("        if (answer[0] == 'refused') != (p.returncode == 1):", "        if False:")]),
    ('A11', PA, 'the limit never ends the wait', [("            if now() - t0 > limit:", "            if False:")]),
    ('A12', PA, 'a publish refused goes on to progress', [("            if verb == 'refused':\n                return f'refused {code} {text}'\n        t0 = now()", "            pass\n        t0 = now()")]),
    # The withheld pick (R10 to R12) and D10
    ('W1', CC, 'withheld_pick without differs (R10/R12 inside it)', [("    v = pick_withheld(ck.records, cut, bps, withholding_changes(ck.records, ck.read, ck.base_read, cut, bps), same)", "    v = pick_withheld(ck.records, cut, bps, None, same)")]),
    ('W2', CC, 'withheld_pick reads the base as zeros (R11b)', [("withholding_changes(ck.records, ck.read, ck.base_read, cut, bps), same)", "withholding_changes(ck.records, ck.read, lambda o, n: bytes(n), cut, bps), same)")]),
    ('W3', CC, 'withheld_pick names nothing passed over', [("    return v, same\n", "    return v, []\n")]),
    ('W4', CC, 'passed_over says nothing', [("    if not same:\n        return ''\n    return (f\" (passed over", "    if True:\n        return ''\n    return (f\" (passed over")]),
    ('W5', CC, "the cut rig's main bypasses withheld_pick (R10 at the call site)", [("        v, same = withheld_pick(ck, mid, bps)", "        v, same = pick_withheld(ck.records, mid, bps), []")]),
    ('W6', NC, "node B bypasses withheld_pick (R12 at the call site)", [("        v, same = cut_check.withheld_pick(ck, cut, bps)", "        v, same = cut_check.pick_withheld(ck.records, cut, bps), []")]),
    ('W7', NC, "node B's lacks line does not name what was passed over (D10)", [("f\"without record {v} ({r['bytes']} bytes at {r['offset']}){passed}\")", "f\"without record {v} ({r['bytes']} bytes at {r['offset']})\")")]),
    ('W8', NC, "node B's files stage ignores a digest that differs (what ctl_files catches)", [("    differ = sorted(k for k in set(want) & set(got) if want[k] != got[k])", "    differ = []")]),
]


def run(cmd, cwd, env=None):
    # No bytecode is read or written: a plant of the same size as the one
    # before it, written in the same second, would otherwise run the
    # earlier plant's cached code.
    shutil.rmtree(os.path.join(cwd, 'tests/kernel/__pycache__'), ignore_errors=True)
    env = dict(env or os.environ, PYTHONDONTWRITEBYTECODE='1')
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=600, env=env)
        return p.returncode, (p.stdout + p.stderr)[-2000:]
    except subprocess.TimeoutExpired:
        return 'timeout', ''


def suites(wt, fn):
    bad = []
    for s in SUITES[fn]:
        rc, out = run([sys.executable, os.path.join('tests/kernel', s)], wt)
        if rc != 0:
            bad.append(f'{s}: ' + ' | '.join(ln for ln in out.splitlines() if ln.startswith(('FAIL:', 'ERROR:')))[:300])
    return bad


def driver(wt):
    env = dict(os.environ, PUBLISH_CHECK_DIR=os.path.join(wt, 'tests/kernel'))
    rc, out = run([sys.executable, '-I', os.path.join(HERE, 'drive_publish_check.py')], wt, env)
    if rc == 0:
        return []
    return [' | '.join(ln for ln in out.splitlines() if ln.startswith(('FAIL:', 'ERROR:')))[:400] or out[-300:]]


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    base = {fn: suites(wt, fn) for fn in SUITES}
    base_d = driver(wt)
    print('baseline', base, base_d, flush=True)
    if any(base.values()) or base_d:
        sys.exit(2)
    results = []
    for pid, fn, what, reps in PLANTS:
        if only and pid not in only:
            continue
        path = os.path.join(wt, fn)
        orig = open(path).read()
        text, status = orig, None
        for old, new in reps:
            if text.count(old) != 1:
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:60]!r})'
                break
            text = text.replace(old, new)
        res = {'id': pid, 'file': fn, 'what': what}
        if status is None:
            try:
                open(path, 'w').write(text)
                bad = suites(wt, fn)
                bad_d = driver(wt) if fn in (PC, NC) else None
            finally:
                open(path, 'w').write(orig)
            res.update(pr_tests='KILLED' if bad else 'SURVIVED', pr_failed=bad,
                       driver='-' if bad_d is None else ('KILLED' if bad_d else 'SURVIVED'), driver_failed=bad_d or [])
        else:
            res.update(pr_tests=status, driver='-')
        clean = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True, text=True).stdout
        res['clean_after'] = clean.strip() == ''
        results.append(res)
        print(f"{pid:4} {res['pr_tests']:9} driver {res['driver']:9} {what}", flush=True)
        json.dump(results, open(outp, 'w'), indent=1)


if __name__ == '__main__':
    main()
