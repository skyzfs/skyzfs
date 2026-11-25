#!/usr/bin/env python3
"""Next look at #145 (M2.T5e), at 9bac3dd and b3bb916 (N40 to N44 are for
b3bb916's line; N23 and N23b are 9bac3dd's text): textual plants, one at a time, each file
put back in a finally, against the suites of the file planted (each
under a 600 s limit). KILLED: a suite failed (a time-out counts);
SURVIVED: every suite passed; NOT APPLIED: an old text is not in the file
exactly once.

- E (aabde27): the cut check's busy-export retry (cut_check.py:
  export_after_import and its caller in Check.kimport), against
  test_cut_check.py.
- N (9bac3dd): the agent's marks (publish_agent.py), read_marks,
  a1_premise, pace_faults and the passist block's new branches
  (publish_check.py).

Usage: python3 -I next_plants.py WORKTREE OUT.json [ID...]
"""
import json
import os
import subprocess
import sys

CC = 'tests/kernel/cut_check.py'
PA = 'tests/kernel/publish_agent.py'
PC = 'tests/kernel/publish_check.py'

T_CC = ['python3', '-I', 'tests/kernel/test_cut_check.py']
T_PA = ['python3', '-I', 'tests/kernel/test_publish_agent.py']
T_PC = ['python3', '-I', 'tests/kernel/test_publish_check.py']
T_PCM = ['python3', '-I', 'tests/kernel/test_publish_check_main.py']
SUITES = {CC: [T_CC], PA: [T_PA, T_PCM], PC: [T_PC, T_PCM]}

LOOP = '    for wait in replay_check.EXPORT_WAITS + (None,):\n'
BUSY = "        if rc == 0 or 'busy' not in out:\n"
LAST = '        if wait is None:\n            return rc, out, refused\n'
CALL = '                erc, eout, refused = export_after_import(POOL)\n'
RAISE = "                if erc:\n                    raise RigError(f'zpool export after the import:"
STRAY_B3 = '    stray = sorted(x for x in written_groups(txgs, w_acc, top) - theirs if x >= g)\n'

PLANTS = [
    # aabde27: the retry.
    ('E1', CC, "no retry: one export, as before aabde27", [(LOOP, '    for wait in (None,):\n')]),
    ('E2', CC, "unbounded: a wait of 1 s for ever", [(LOOP, '    for wait in iter(lambda: 1, None):\n')]),
    ('E3', CC, "one wait more (16 s)", [(LOOP, '    for wait in replay_check.EXPORT_WAITS + (16, None):\n')]),
    ('E4', CC, "every failure tried again, not only a busy one", [(BUSY, '        if rc == 0:\n')]),
    ('E5', CC, "a busy export not tried again, any other failure is", [(BUSY, "        if rc == 0 or 'busy' in out:\n")]),
    ('E6', CC, "no wait between tries", [('        sleep(wait)\n', '        pass\n')]),
    ('E7', CC, "a wait of 1 s every time", [('        sleep(wait)\n', '        sleep(1)\n')]),
    ('E8', CC, "no refusal recorded", [("        refused.append(f'busy", "        [].append(f'busy")]),
    ('E9', CC, "no try after the last wait", [(LOOP, '    for wait in replay_check.EXPORT_WAITS:\n')]),
    ('E10', CC, "still busy after the last wait returned as exported", [(LAST, '        if wait is None:\n            return 0, out, refused\n')]),
    ('E11', CC, "kimport: a failed export no longer the rig's error", [(RAISE, RAISE.replace('if erc:', 'if False:'))]),
    ('E12', CC, "kimport: the retry bypassed (one export, as before)",
     [(CALL, "                erc, eout, _ = run(['zpool', 'export', POOL], 120)\n                refused = []\n")]),
    ('E13', CC, "kimport: a busy export after all waits passes as exported",
     [(CALL, CALL + "                erc = 0 if refused else erc\n")]),
    # 9bac3dd: the agent's marks.
    ('N1', PA, "no accepted.tsv", [("            if assist_after is not None:\n                with open(os.path.join(out_dir, 'accepted.tsv')",
                                    "            if False:\n                with open(os.path.join(out_dir, 'accepted.tsv')")]),
    ('N2', PA, "the acceptance mark's A not the wait", [("        mark('accepted', assist_after)", "        mark('accepted', 1.0)")]),
    ('N3', PA, "no end mark for a refusal", [("                return ended(f'refused {code} {text}')", "                return f'refused {code} {text}'")]),
    ('N4', PA, "no end mark for an end", [("                return ended(verb + ''.join(", "                return (verb + ''.join(")]),
    ('N5', PA, "no end mark for pending after", [("                return ended(f'pending after {limit} s')", "                return f'pending after {limit} s'")]),
    ('N6', PA, "the end mark with the whole end line", [("            mark('end', end.split(' ', 1)[0])", "            mark('end', end.replace(' ', '_'))")]),
    ('N7', PA, "the mark's W not read from the kstat", [("{mono()}\\t{synced(pool)}\\t{last}", "{mono()}\\t0\\t{last}")]),
    ('N8', PA, "the mark's time 0", [("{mono()}\\t{synced(pool)}\\t{last}", "0\\t{synced(pool)}\\t{last}")]),
    # 9bac3dd: read_marks.
    ('N10', PC, "read_marks takes a mark twice", [("            and f[1] not in out.get(f[0], {}) and bool(f[4])", "            and bool(f[4])")]),
    ('N11', PC, "read_marks takes a wait of 0", [("            ok = ok and last > 0\n", "")]),
    # 9bac3dd: a1_premise.
    ('N12', PC, "a1_premise: no end mark holds", [("    if 'end' not in marks:\n        return False,", "    if 'end' not in marks:\n        return True,")]),
    ('N13', PC, "a1_premise: top from the end mark alone", [("    top = max([w_end] + [r['after'] for r in rows])", "    top = w_end")]),
    ('N14', PC, "a1_premise: top from the assists alone", [("    top = max([w_end] + [r['after'] for r in rows])", "    top = max([0] + [r['after'] for r in rows])")]),
    ('N15', PC, "a1_premise: G at the acceptance allowed", [("    if w_acc >= g:", "    if w_acc > g:")]),
    ('N16', PC, "a1_premise: G reached read as below", [("    if top < g:", "    if top <= g:")]),
    ('N17', PC, "a1_premise: below G, pending holds", [("        if verb == 'pending':\n            return False,", "        if verb == 'pending':\n            return True,")]),
    ('N18', PC, "a1_premise: below G, any end the rig's", [("        if verb == 'pending':\n", "        if True:\n")]),
    ('N19', PC, "a1_premise: crossing with <= at before", [("if r['before'] < g <= r['after']]", "if r['before'] <= g <= r['after']]")]),
    ('N20', PC, "a1_premise: no crossing needed", [("    if not crossed:\n        return False,", "    if False:\n        return False,")]),
    ('N21', PC, "a1_premise: strays not judged", [("    if stray:\n", "    if False:\n")]),
    ('N22', PC, "a1_premise: an assist's range from its before", [("range(r['before'] + 1, r['after'] + 1)", "range(r['before'], r['after'] + 1)")]),
    ('N23', PC, "a1_premise: strays only at or above G (B1's sketch)",
     [("    stray = sorted(written_groups(txgs, w_acc, top) - theirs)", "    stray = sorted(x for x in written_groups(txgs, w_acc, top) - theirs if x >= g)")]),
    ('N23b', PC, "a1_premise: strays from the first assist on (B1's fix, the smaller form)",
     [("    stray = sorted(written_groups(txgs, w_acc, top) - theirs)",
       "    stray = sorted(written_groups(txgs, rows[0]['before'] if rows else w_acc, top) - theirs)")]),
    # b3bb916: strays only at or above G (the text of 9bac3dd's N23 is gone).
    ('N40', PC, "b3bb916: a stray at G itself not counted", [(STRAY_B3, STRAY_B3.replace('if x >= g)', 'if x > g)'))]),
    ('N41', PC, "b3bb916 undone: every stray counted, as at 9bac3dd", [(STRAY_B3, STRAY_B3.replace(' if x >= g)', ')'))]),
    ('N42', PC, "b3bb916: only strays below G counted", [(STRAY_B3, STRAY_B3.replace('if x >= g)', 'if x < g)'))]),
    ('N43', PC, "b3bb916 as the form 'first': strays from the first assist on",
     [(STRAY_B3, "    stray = sorted(written_groups(txgs, rows[0]['before'] if rows else w_acc, top) - theirs)\n")]),
    ('N44', PC, "b3bb916: only a stray at the top counted", [(STRAY_B3, STRAY_B3.replace('if x >= g)', 'if x >= top)'))]),
    # 9bac3dd: pace_faults.
    ('N24', PC, "pace: no first-assist check", [("    if rows and rows[0]['t0'] - accepted_ns < after * 1e9:", "    if False:")]),
    ('N25', PC, "pace: the burst's bound 2 A", [("gap >= after / 2:", "gap >= after * 2:")]),
    ('N26', PC, "pace: the spaced bound A / 2", [("gap < after * 0.95:", "gap < after * 0.5:")]),
    ('N27', PC, "pace: a burst of two", [("        if k < publish_agent.ASSIST_BURST and gap", "        if k < 2 and gap"),
                                         ("        if k >= publish_agent.ASSIST_BURST and gap", "        if k >= 2 and gap")]),
    ('N28', PC, "pace: a burst of four", [("        if k < publish_agent.ASSIST_BURST and gap", "        if k < 4 and gap"),
                                          ("        if k >= publish_agent.ASSIST_BURST and gap", "        if k >= 4 and gap")]),
    ('N29', PC, "pace: the decided A for the mark's", [("                                       marks[t]['accepted'][2])", "                                       publish_agent.ASSIST_AFTER)")]),
    # 9bac3dd: the passist block.
    ('N30', PC, "a2's pace not judged", [("            for t in ('a1', 'a2'):", "            for t in ('a1',):")]),
    ('N31', PC, "a1 never accepted holds whatever its end",
     [("                premise('premise_a1_needed_the_assist', ends.get('a1', '').startswith('refused '),",
       "                premise('premise_a1_needed_the_assist', True,")]),
    ('N32', PC, "the NA branch never taken", [("            if not ends.get('a2', '').startswith('pending after'):", "            if False:")]),
    ('N33', PC, "the NA branch always taken", [("            if not ends.get('a2', '').startswith('pending after'):", "            if True:")]),
    ('N34', PC, "premise_a2 with >= 1", [("len(a2) >= 2, f'a2 was assisted", "len(a2) >= 1, f'a2 was assisted")]),
    ('N35', PC, "accepted.tsv required", [("marks = read_marks(text_of('accepted.tsv', need=False))", "marks = read_marks(text_of('accepted.tsv'))")]),
    ('N36', PC, "the statuses required", [("status_of(text_of('a2-status-after.txt', need=False))", "status_of(text_of('a2-status-after.txt'))")]),
    ('N37', PC, "lacks: only a missing key", [("for k in ('uploaded', 'upload_blocks') if not st.get(k, '').isdigit()})",
                                              "for k in ('uploaded', 'upload_blocks') if k not in st})")]),
]


def suites(wt, fn):
    bad = []
    for cmd in SUITES[fn]:
        try:
            p = subprocess.run(cmd, cwd=wt, capture_output=True, text=True, timeout=600)
            if p.returncode != 0:
                bad.append(os.path.basename(cmd[-1]))
        except subprocess.TimeoutExpired:
            bad.append(os.path.basename(cmd[-1]) + ' (TIMEOUT)')
    return bad


def main():
    wt, outp = sys.argv[1], sys.argv[2]
    only = set(sys.argv[3:])
    files = {fn for pid, fn, _, _ in PLANTS if not only or pid in only}
    base = {fn: suites(wt, fn) for fn in files}
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
                status = f'NOT APPLIED ({text.count(old)} matches of {old[:50]!r})'
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
            res.update(result='KILLED' if bad else 'SURVIVED', by=bad)
        else:
            res.update(result=status)
        res['clean_after'] = subprocess.run(['git', 'status', '--porcelain'], cwd=wt, capture_output=True,
                                            text=True).stdout.strip() == ''
        results.append(res)
        print(f"{pid:4} {res['result']:9} {what}{'  by ' + ', '.join(res.get('by', [])) if res.get('by') else ''}",
              flush=True)
        with open(outp, 'w') as f:
            json.dump(results, f, indent=1)
    after = {fn: suites(wt, fn) for fn in files}
    print('restored', after, flush=True)


if __name__ == '__main__':
    main()
