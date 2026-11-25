#!/usr/bin/env python3
"""Review of #143 (M2.T5d) at d3b75af: textual plants, one at a time, each
file put back in a finally, against the suites of the file planted (each
under a 600 s limit). KILLED: a suite failed (a time-out counts);
SURVIVED: every suite passed; NOT APPLIED: an old text is not in the file
exactly once.

- A: N3's anchor (asked_line), V: pin_watcher_reads, C: prewindc and the
  new looks in publish_check.py, S: the courses in cut-record.sh and the
  workflow, W: the wiring tests of cut_check.main and nodeb_check.main
  (W5 to W7 of #139 re-aimed, then the asked questions: a call dropped,
  reordered, given the wrong argument, wrapped in dead code), H: harmless
  refactors of those mains (a KILL there is a false failure), B: the
  bench's new seed rule and clause.

Usage: python3 -I plants143.py WORKTREE OUT.json [ID...]
"""
import json
import os
import subprocess
import sys

PC = 'tests/kernel/publish_check.py'
SH = 'tests/kernel/cut-record.sh'
WF = '.github/workflows/skyzfs-publish-rig.yml'
CK = 'tests/kernel/cut_check.py'
NB = 'tests/kernel/nodeb_check.py'
BK = 'tests/bench/bracket.py'
BS = 'tests/bench/topology-bench.sh'

T_PC = ['python3', '-I', 'tests/kernel/test_publish_check.py']
T_PCM = ['python3', '-I', 'tests/kernel/test_publish_check_main.py']
T_REC = ['bash', 'tests/kernel/test_cut_record.sh']
T_CK = ['python3', '-I', 'tests/kernel/test_cut_check.py']
T_NB = ['python3', '-I', 'tests/kernel/test_nodeb_check.py']
T_BRK = ['python3', '-I', 'tests/bench/test_bracket.py']
T_EMB = ['python3', '-I', 'tests/bench/test_embedded_python.py']
T_COPY = ['bash', 'tests/bench/test_copy_limit.sh']
SUITES = {PC: [T_PC, T_PCM], SH: [T_REC], WF: [T_REC], CK: [T_CK, T_NB], NB: [T_NB, T_CK],
          BK: [T_BRK, T_EMB], BS: [T_COPY, T_EMB]}

PICK_CK = "        v, same = withheld_pick(ck, mid, bps)\n"
PASS_CK = "        passed = passed_over(same)\n"
PICK_NB = "        v, same = cut_check.withheld_pick(ck, cut, bps)\n"
PREMC = "premise('premise_c1_waited_across_a_plain_import', found and bool(later) and not grounds,"
SINCE = "and int(ln['sectors']) > after and ln['ns'] > since):"
WATCH = ("        watch = text_of('watch.log').splitlines()\n"
         "        err = [ln.split('\\t', 1)[-1] for ln in watch if '\\tERROR ' in ln]\n"
         "        if err:\n"
         "            raise RigError(f'the watcher: {err[0]}')\n"
         "        bad = [ln.split('\\tVERDICT ', 1)[1] for ln in watch if '\\tVERDICT ' in ln]\n"
         "        holds('pin_watcher_reads', bad, 'kernel and server',\n"
         "              f'node B read the content of every cut the watcher found at head ({len(watch)} lines)')\n")
RC_BODY = ("    pflow c1 c1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n"
           "    export_pool\n"
           "    import_pool\n"
           "    pwait c1-end c1 60 sync\n")

PLANTS = [
    # N3: the first ask's own line.
    ('A1', PC, "asked_line without C (N3 undone)",
     [("\n                       and ln['sectors'] == str(after))", ")")]),
    ('A2', PC, "the anchor on the ask anew's C", [("asked_line('r1', r1['after'])", "asked_line('r1', published['r1']['after'])")]),
    ('A3', PC, "C read from the sector column (G)", [("and ln['sectors'] == str(after))", "and ln['sector'] == str(after))")]),
    # pin_watcher_reads.
    ('V1', PC, "watch.log not read (the state before the PR)", [(WATCH, "")]),
    ('V2', PC, "the watcher's VERDICT lines not judged", [("holds('pin_watcher_reads', bad,", "holds('pin_watcher_reads', [],")]),
    ('V3', PC, "a watcher's ERROR line not the rig's error",
     [("        if err:\n            raise RigError(f'the watcher: {err[0]}')\n", "")]),
    ('V4', PC, "no watch.log taken as no verdict", [("text_of('watch.log')", "text_of('watch.log', need=False)")]),
    ('V5', PC, "the pin only in courses that publish",
     [("holds('pin_watcher_reads', bad,", "holds('pin_watcher_reads', bad if PUBLISHED[name] else [],")]),
    ('V6', PC, "the VERDICT match without its tab", [("for ln in watch if '\\tVERDICT ' in ln]", "for ln in watch if 'VERDICT ' in ln]")]),
    # prewindc's premise and tables; the new looks.
    ('C1', PC, "prewindc's premise without 'no ground' (a rewind passes)", [(PREMC, PREMC.replace(' and not grounds', ''))]),
    ('C2', PC, "prewindc's premise without the cuts after the import", [(PREMC, PREMC.replace(' and bool(later)', ''))]),
    ('C3', PC, "cuts_since without the mark's time", [(SINCE, "and int(ln['sectors']) > after):")]),
    ('C4', PC, "cuts_since without C", [(SINCE, "and ln['ns'] > since):")]),
    ('C5', PC, "the import's mark taken from the create (session-1-ready)",
     [("if m == 'session-2-ready']", "if m == 'session-1-ready']")]),
    ('C6', PC, "withdraw_line on a name's prefix", [("and ln['result'].split(' ', 1)[0] == name)", "and ln['result'].startswith(name))")]),
    ('C7', PC, "withdraw_line on any publish op", [("and ln['op'] == 'withdraw'\n", "and ln['op'] in ('withdraw', 'ended', 'asked')\n")]),
    ('C8', PC, "c1-end may be refused rewound", [("'c1-end': ('pending after',),", "'c1-end': ('pending after', 'refused rewound'),")]),
    ('C9', PC, "c1-withdraw may be unknown or refused", [("'c1-withdraw': ('withdrawn',),", "'c1-withdraw': ('withdrawn', 'unknown', 'refused'),")]),
    ('C10', PC, "ABSENT without c1", [("'prewindc': ('c1',), ", "")]),
    ('C11', PC, "COURSES without prewindc", [("'prewindf', 'prewindc', 'pkill'", "'prewindf', 'pkill'")]),
    ('C12', PC, "f1-look may find a snapshot", [("'f1-look': ('absent',),", "'f1-look': ('absent', 'snap '),")]),
    ('C13', PC, "tw-look may find a snapshot", [("'tw-look': ('absent',),", "'tw-look': ('absent', 'snap '),")]),
    ('C14', PC, "c1-look not in the table", [("        'c1-look': ('absent',),\n", "")]),
    ('C15', PC, "prewindc's grounds over the whole log, no stop",
     [("found, grounds = rewind_cuts(instances, c1['after'], withdraw_line('c1'))",
       "found, grounds = True, rewind_cuts(instances, c1['after'])[1]")]),
    # The courses (cut-record.sh) and the workflow.
    ('S1', SH, "prewindc imports with import_rewind_f", [(RC_BODY, RC_BODY.replace('    import_pool\n', '    import_rewind_f\n'))]),
    ('S2', SH, "prewindc zeroes the root block (a rewind)", [(RC_BODY, RC_BODY.replace('    import_pool\n', '    zero_newest_root\n    import_rewind_f\n'))]),
    ('S3', SH, "prewindc: no export and import at all", [(RC_BODY, RC_BODY.replace('    export_pool\n    import_pool\n', ''))]),
    ('S4', SH, "prewindc: c1 asked after the import",
     [(RC_BODY, "    export_pool\n    import_pool\n    pflow c1 c1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n"
                "    pwait c1-end c1 60 sync\n")]),
    ('S5', SH, "prewindc: c1's end read with no syncs", [("    pwait c1-end c1 60 sync\n", "    pwait c1-end c1 60\n")]),
    ('S6', SH, "prewindc: c1's end read after 6 s", [("    pwait c1-end c1 60 sync\n", "    pwait c1-end c1 6 sync\n")]),
    ('S7', SH, "prewindc: c1 expires in 30 s",
     [("    pflow c1 c1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n", "    pflow c1 c1 --group-plus 1000000 --expiry-s 30 --limit-s 3\n")]),
    ('S8', SH, "prewindc: c1 asked with G a group above (it could be published)",
     [("    pflow c1 c1 --group-plus 1000000 --expiry-s 900 --limit-s 3\n", "    pflow c1 c1 --group-plus 1 --expiry-s 900 --limit-s 3\n")]),
    ('S9', SH, "prewindc: no withdrawal", [("    pask c1-withdraw withdraw c1\n", "")]),
    ('S10', SH, "PUBLISH_NAMED without prewindc", [('PUBLISH_NAMED="pplain prewind prewindf prewindc pkill', 'PUBLISH_NAMED="pplain prewind prewindf pkill')]),
    ('S11', SH, "prewindf: f1's look dropped", [("    pask f1-look look f1\n", "")]),
    ('S12', SH, "pkill: tw's look dropped", [("    pask tw-look look tw\n", "")]),
    ('S13', SH, "pkill: tw's look after the kill",
     [("    pask tw-look look tw\n    pflow tk tk --group-plus 30 --expiry-s 900 --limit-s 3\n    kill_serve\n    serve_start 2\n",
       "    pflow tk tk --group-plus 30 --expiry-s 900 --limit-s 3\n    kill_serve\n    serve_start 2\n    pask tw-look look tw\n")]),
    ('S14', WF, "the workflow's default list without prewindc",
     [('"${COURSES:-pplain prewind prewindf prewindc pkill pkillman pkillpub pchannel}"',
       '"${COURSES:-pplain prewind prewindf pkill pkillman pkillpub pchannel}"')]),
    # W5 to W7 of #139, re-aimed at the wiring as it stands.
    ('W5', CK, "cut_check.main bypasses withheld_pick", [(PICK_CK, "        v, same = pick_withheld(ck.records, mid, bps), []\n")]),
    ('W6', NB, "nodeb_check.main bypasses withheld_pick", [(PICK_NB, "        v, same = cut_check.pick_withheld(ck.records, cut, bps), []\n")]),
    ('W7', NB, "node B's lacks line does not name what was passed over",
     [("f\"without record {v} ({r['bytes']} bytes at {r['offset']}){passed}\")", "f\"without record {v} ({r['bytes']} bytes at {r['offset']})\")")]),
    ('W7b', CK, "the cut rig's built withheld line does not name it",
     [("f\"{r['offset']}){passed}: {failed or 'passed'}: {detail}\")", "f\"{r['offset']}): {failed or 'passed'}: {detail}\")")]),
    ('W7c', NB, "node B's not-built lacks line does not name it", [("                            f'reaches{passed}')", "                            'reaches')")]),
    # The asked questions on the wiring tests.
    ('W8', CK, "cut_check: the call dropped (no pick)", [(PICK_CK, "        v, same = None, []\n")]),
    ('W9', NB, "nodeb: the call dropped (no pick)", [(PICK_NB, "        v, same = None, []\n")]),
    ('W10', CK, "cut_check: the early cut passed", [(PICK_CK, PICK_CK.replace('ck, mid, bps', 'ck, early, bps'))]),
    ('W11', CK, "cut_check: the arguments reordered", [(PICK_CK, PICK_CK.replace('ck, mid, bps', 'ck, bps, mid'))]),
    ('W12', NB, "nodeb: another list of blocks", [(PICK_NB, PICK_NB.replace('ck, cut, bps', 'ck, cut, []'))]),
    ('W13', CK, "cut_check: passed_over given an empty list", [(PASS_CK, "        passed = passed_over([])\n")]),
    ('W14', CK, "cut_check: the pick wrapped in dead code (never called)",
     [(PICK_CK, "        if False:\n            v, same = withheld_pick(ck, mid, bps)\n        else:\n            v, same = None, []\n")]),
    ('W15', NB, "nodeb: the pick wrapped in dead code (never called)",
     [(PICK_NB, "        if False:\n            v, same = cut_check.withheld_pick(ck, cut, bps)\n        else:\n            v, same = None, []\n")]),
    ('W16', CK, "cut_check: v taken again from pick_later after the pick",
     [(PICK_CK, PICK_CK + "        v = pick_later(ck.records, mid, bps)\n")]),
    ('W17', CK, "cut_check: the schedule keeps the picked write", [("            sched = [i for i in range(mid['prefix']) if i != v]\n",
                                                               "            sched = list(range(mid['prefix']))\n")]),
    ('W18', NB, "nodeb: the image keeps the picked write", [("image_at(0, [i for i in range(cut['prefix']) if i != v])",
                                                          "image_at(0, list(range(cut['prefix'])))")]),
    ('W19', CK, "cut_check: passed_over before the pick (reordered)", [(PICK_CK + PASS_CK, PASS_CK + PICK_CK)]),
    ('W20', CK, "cut_check: passed over the first eight only", [(PASS_CK, "        passed = passed_over(same[:1])\n")]),
    ('W21', CK, "cut_check: the not-built branch taken whenever a write was passed over",
     [("        if v is None:\n            got['withheld'] = ('not built'", "        if v is None or same:\n            got['withheld'] = ('not built'")]),
    # Harmless refactors (a KILL is a false failure).
    ('H1', CK, "cut_check: keyword arguments", [(PICK_CK, PICK_CK.replace('ck, mid, bps', 'ck, cut=mid, bps=bps'))]),
    ('H2', CK, "cut_check: `same` renamed `skipped`", [(PICK_CK + PASS_CK, PICK_CK.replace('same', 'skipped') + PASS_CK.replace('same', 'skipped'))]),
    ('H3', CK, "cut_check: bps read from bps_at", [(PICK_CK, PICK_CK.replace('ck, mid, bps', "ck, mid, bps_at['mid']"))]),
    ('H4', CK, "cut_check: the pick's pair unpacked on a line of its own",
     [(PICK_CK, "        pick = withheld_pick(ck, mid, bps)\n        v, same = pick\n")]),
    ('H5', NB, "nodeb: withheld_pick imported by name",
     [(PICK_NB, "        from cut_check import withheld_pick\n        v, same = withheld_pick(ck, cut, bps)\n")]),
    # The bench's seed rule and clause.
    ('B1', BK, "seed_row without the found-committed test", [("    if str(sb.get('upload_found_committed', '0')) != '0':\n        return None\n", "")]),
    ('B2', BK, "a status without the key read as found", [("sb.get('upload_found_committed', '0')", "sb.get('upload_found_committed', '1')")]),
    ('B3', BK, "a non-number read as none found",
     [("    if str(sb.get('upload_found_committed', '0')) != '0':", "    if str(sb.get('upload_found_committed', '0')).isdigit() and str(sb.get('upload_found_committed', '0')) != '0':")]),
    ('B4', BK, "found-committed read at the window's end", [("    if str(sb.get('upload_found_committed', '0')) != '0':", "    if str((k.get('after') or {}).get('upload_found_committed', '0')) != '0':")]),
    ('B5', BK, "the clause names the lease held at the start", [("if lease[1][0] in LEASE_HELD\n", "if lease[0][0] in LEASE_HELD\n")]),
    ('B6', BK, "the clause always says held",
     [("            under = ('under the lease held at its end' if lease[1][0] in LEASE_HELD\n                     else 'under a lease, since none was held at its end')\n",
       "            under = 'under the lease held at its end'\n")]),
    ('B7', BS, "the paragraph without the found-committed condition",
     [("'ran, and whose daemon had found no cut committed already) keeps", "'ran) keeps")]),
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
    print('baseline', {k: v for k, v in base.items()}, flush=True)
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
