#!/usr/bin/env python3
"""Plants for the review of #171 at 4ccc4b4: each applied alone to a
worktree (argv[1]), the suite that should catch it run under a 600 s
limit, the file restored with `git checkout`. Run with python3 -I.
argv[2:]: plant ids to run (all where none)."""
import subprocess
import sys

WT = sys.argv[1]
ONLY = set(sys.argv[2:])
PY = ['python3', '-I', 'tests/kernel/test_agent_check.py']
AR = ['bash', 'tests/kernel/test_agent_record.sh']
ZR = ['bash', 'tests/kernel/test_zfs_cli_record.sh']
AC = 'tests/kernel/agent_check.py'
AS = 'tests/kernel/agent-record.sh'
ZS = 'tests/kernel/zfs-cli-record.sh'
CS = 'tests/kernel/cut-record.sh'

PLANTS = [
    # agent_check.py: the rewind premise and pin_R-rewind_synced_past.
    ('A01', 'premise: no opens=rewind check', AC, "    if opens != 'rewind':\n", "    if False:\n", PY),
    ('A02', 'premise: no top-above-import check', AC, '    if tg is None or tg <= t:\n', '    if tg is None:\n', PY),
    ('A03', 'premise: top equal to import accepted', AC, '    if tg is None or tg <= t:\n', '    if tg is None or tg < t:\n', PY),
    ('A04', 'synced: 0 accepted', AC, "    elif first['synced'] == 0:\n", "    elif first['synced'] == -1:\n", PY),
    ('A05', 'synced: no synced= accepted', AC, "    if first['synced'] is None:\n", '    if False:\n', PY),
    ('A06', 'synced: publish not printed accepted', AC, "    elif first['begun'] not in published:\n", '    elif False:\n', PY),
    ('A07', 'synced: later syncs accepted', AC, "    later = [r['n'] for r in rounds[1:] if r['synced']]\n", '    later = []\n', PY),
    ('A08', 'synced: "no publish begun" ignored', AC, '    said = [ln for ln in err.splitlines() if NO_BEGIN in ln]\n', '    said = []\n', PY),
    ('A09', 'synced: pin judged whatever the premise', AC, "                   'rewind'):\n", "                   'rewind') or True:\n", PY),
    ('A10', 'premise always holds', AC, '    return bad\n\n\ndef judge_synced(', '    return []\n\n\ndef judge_synced(', PY),
    ('A11', 'synced: the last round judged as the first', AC, '    first, bad = rounds[0], []\n', '    first, bad = rounds[-1], []\n', PY),
    # 4ccc4b4: the fields after under_way.
    ('A12', 'round line: positional synced= again', AC, "((?: [a-z][a-z_]*=[^ =]+)*)')", "((?: synced=[0-9]+)?)')", PY),
    ('A13', 'round line: a field twice accepted', AC, '        if k in out:\n', '        if False:\n', PY),
    ('A14', 'round line: only the first field read', AC, '    for f in more.split():\n', '    for f in more.split()[:1]:\n', PY),
    ('A15', 'round line: synced never read', AC, "'synced': int(more['synced']) if 'synced' in more else None})",
     "'synced': None})", PY),
    ('A16', 'round line: synced not checked a count', AC, "    if 'synced' in out and not out['synced'].isdigit():\n",
     '    if False:\n', PY),
    # agent_check.py: the run course.
    ('A17', 'runkill not KILLED_ONLY', AC, "KILLED_ONLY = ('gone', 'both', 'runkill')", "KILLED_ONLY = ('gone', 'both')", PY),
    ('A18', 'END_WITHIN 20 s', AC, 'END_WITHIN = 2.0', 'END_WITHIN = 20.0', PY),
    ('A19', 'TICK_SLACK 5 s', AC, 'TICK_SLACK = 1.0', 'TICK_SLACK = 5.0', PY),
    ('A20', 'rounds: begun=- accepted', AC, '    if None in names:\n', '    if False:\n', PY),
    ('A21', 'runrewind: one round enough', AC,
     "judge_rounds(rounds, read_arrivals(text(at('publish.rounds.tsv'))), 2), 'server',\n              f'{len(rounds)} rounds after",
     "judge_rounds(rounds, read_arrivals(text(at('publish.rounds.tsv'))), 1), 'server',\n              f'{len(rounds)} rounds after", PY),
    ('A22', "runrewind: no end's pin", AC,
     " rounds after the rewind import, each ok and beginning its own publish')\n        ended('publish', 'sigterm')\n",
     " rounds after the rewind import, each ok and beginning its own publish')\n", PY),
    ('A23', 'run2 not read', AC, "for run in ('publish', 'first', 'run2', 'settle-frozen'", "for run in ('publish', 'first', 'settle-frozen'", PY),
    ('A24', "runbad: a last line 'ok' accepted", AC, "int(r[1]) != n or r[2] != 'failed':", 'int(r[1]) != n:', PY),
    ('A25', 'runheld: no round after the refusals accepted', AC, 'int(before) >= len(rounds):', 'int(before) > len(rounds):', PY),
    ('A26', "arrivals' numbering not checked", AC, 'ok = len(f) == 2 and int(f[0]) == k\n', 'ok = len(f) == 2 and int(f[0]) > 0\n', PY),
    ('A27', 'rounds: skipped when in time accepted', AC, "            if a['skipped']:\n", '            if False:\n', PY),
    ('A28', "FIRST_RC runrewind '1'", AC, "    'runrewind': '0',\n}", "    'runrewind': '1',\n}", PY),
    # agent-record.sh: R-rewind and the run scenarios.
    ('S01', 'runrewind: the writer on before the run', AS,
     '    abg publish run --every-s "$RUN_EVERY" "$@"\n    await_rounds publish 1 300 || true\n    writer_on\n',
     '    writer_on\n    abg publish run --every-s "$RUN_EVERY" "$@"\n    await_rounds publish 1 300 || true\n', AR),
    ('S02', 'runrewind: no thaw', AS, '    import_pool -T "$t"\n    thaw\n', '    import_pool -T "$t"\n', AR),
    ('S03', 'runrewind: thaw before the import', AS, '    import_pool -T "$t"\n    thaw\n', '    thaw\n    import_pool -T "$t"\n', AR),
    ('S04', 'runrewind: one group further back', AS, 't=${groups[$(( ${#groups[@]} - REWIND_BACK ))]}',
     't=${groups[$(( ${#groups[@]} - REWIND_BACK - 1 ))]}', AR),
    ('S05', 'rewind_groups: eight groups enough', AS, '        [ "${#REWIND_GROUPS[@]}" -le "$REWIND_BACK" ] || break\n',
     '        [ "${#REWIND_GROUPS[@]}" -lt "$REWIND_BACK" ] || break\n', AR),
    ('S06', 'runrewind: no freeze', AS, '    writer_off\n    freeze\n    first=$(committed | tail -n 1)\n',
     '    writer_off\n    first=$(committed | tail -n 1)\n', AR),
    ('S07', 'runrewind: one round then SIGTERM', AS, '    writer_on\n    run_rounds_then_term publish 2\n}',
     '    writer_on\n    run_rounds_then_term publish 1\n}', AR),
    ('S08', 'runrewind: no export', AS, '    export_pool\n    pagent ask cuts > "$CDIR/rewind-top.txt"',
     '    pagent ask cuts > "$CDIR/rewind-top.txt"', AR),
    ('S09', 'rewind_groups: the first group counted', AS, "groups_written | awk -v g=\"$1\" '$1 > g'",
     "groups_written | awk -v g=\"$1\" '$1 >= g'", AR),
    ('S10', 'runbad: acts after any round', AS, '        [ "${left:-0}" -lt 3000 ] || break\n',
     '        [ "${left:-0}" -lt 0 ] || break\n', AR),
    ('S11', 'runtwice: SIGTERM alone', AS,
     '        kill -TERM "$AGENT_PID" 2>/dev/null || true\n        kill -INT "$AGENT_PID" 2>/dev/null || true\n',
     '        kill -TERM "$AGENT_PID" 2>/dev/null || true\n', AR),
    ('S12', "await_rounds: a constant stamp", AS, "printf '%s\\t%s\\n' \"$k\" \"$EPOCHREALTIME\" >> \"$CDIR/$label.rounds.tsv\"",
     "printf '%s\\t%s\\n' \"$k\" 0 >> \"$CDIR/$label.rounds.tsv\"", AR),
    ('S13', 'runrewind: rewind-cuts read before the thaw', AS,
     '    thaw\n    pagent ask cuts > "$CDIR/rewind-cuts.txt"',
     '    pagent ask cuts > "$CDIR/rewind-cuts.txt"', AR),
    # zfs-cli-record.sh (4043629) and Q04.
    ('Z01', 'rate_of inverted', ZS, 'else printf "%.2f\\n", g / s }', 'else printf "%.2f\\n", s / g }', ZR),
    ('Z02', 'read rate over the idle window', ZS, 'rr+=("$(rate_of "${rt[-1]}" "${rw[-1]}")")', 'rr+=("$(rate_of "${rt[-1]}" "${iw[-1]}")")', ZR),
    ('Z03', 'no cut rounds', ZS, 'if [ "$(cat "$OUT/$r/rc" 2> /dev/null)" = 124 ]; then', 'if false; then', ZR),
    ('Z04', "cut read from the idle side's rc", ZS, 'if [ "$(cat "$OUT/$r/rc" 2> /dev/null)" = 124 ]; then',
     'if [ "$(cat "$OUT/$c/rc" 2> /dev/null)" = 124 ]; then', ZR),
    ('Z05', 'busy is the last run', ZS, '\'BEGIN { printf "%.3f\\n", x + y }\')', '\'BEGIN { printf "%.3f\\n", y }\')', ZR),
    ('Z06', "share over the idle window", ZS, 'rb+=("$(share_of "$(field_of "$r" stdout busy)" "${rw[-1]}")")',
     'rb+=("$(share_of "$(field_of "$r" stdout busy)" "${iw[-1]}")")', ZR),
    ('Z07', 'budget: settle off still budgeted', ZS, 'if [[ ! $st$ss =~ ^[0-9]+$ ]] || [[ $st =~ ^0+$ ]]; then',
     'if [[ ! $st$ss =~ ^[0-9]+$ ]]; then', ZR),
    ('Z08', 'budget: seven digits as 30 s', ZS, '            ss=999999\n', '            ss=30\n', ZR),
    ('Z09', "writing's default margin 3", ZS, 'margin=${ZCR_WRITER_MARGIN:-2}', 'margin=${ZCR_WRITER_MARGIN:-3}', ZR),
    ('Z10', "budget's default margin 3", ZS, '        m=${ZCR_WRITER_MARGIN:-2}\n', '        m=${ZCR_WRITER_MARGIN:-3}\n', ZR),
    ('Z11', 'release_quietly releases nothing (P18)', ZS, '        zfs release "$TAG" "$s" > /dev/null 2>&1\n',
     '        : "$s"\n', ZR),
    ('Z12', 'release_quietly unanchored', ZS, 'grep "@$2\\$"', 'grep "@$2"', ZR),
    ('Z13', 'cut round: idle side kept', ZS, 'rb+=(cut) rl+=(cut) il+=(cut)', 'rb+=(cut) rl+=(cut) il+=("$(stall_of "$c")")', ZR),
    ('Q04', 'end_daemon: pid let go after rig_error', CS,
     'serve_end 10 || { SERVE_PID=""; rig_error "skyzfsd serve did not end 10 s after SIGKILL"; }',
     'serve_end 10 || { rig_error "skyzfsd serve did not end 10 s after SIGKILL"; SERVE_PID=""; }', AR),
]


def run(cmd):
    try:
        p = subprocess.run(cmd, cwd=WT, capture_output=True, text=True, timeout=600)
        return p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return 'timeout', ''


def failing(out):
    keep = [ln for ln in out.splitlines() if ln.startswith(('FAIL', 'ERROR:', 'AssertionError', 'FAILED'))]
    return keep[:3]


for pid, what, path, old, new, suite in PLANTS:
    if ONLY and pid not in ONLY:
        continue
    full = f'{WT}/{path}'
    with open(full) as f:
        text = f.read()
    n = text.count(old)
    if n != 1:
        print(f'{pid}\t{what}\tNOT APPLIED: the text occurs {n} times', flush=True)
        continue
    with open(full, 'w') as f:
        f.write(text.replace(old, new))
    try:
        rc, out = run(suite)
        passed = rc == 0 and ('all passed' in out or suite is PY or 'FAIL' not in out)
        if suite is ZR:
            passed = rc == 0 and 'FAIL' not in out
        if suite is AR:
            passed = rc == 0 and 'all passed' in out
        verdict = 'SURVIVES' if passed else 'killed'
        print(f'{pid}\t{what}\t{verdict}\t{" | ".join(failing(out))[:400]}', flush=True)
    finally:
        subprocess.run(['git', 'checkout', '--', path], cwd=WT, check=True)
        d = subprocess.run(['git', 'status', '--porcelain'], cwd=WT, capture_output=True, text=True).stdout
        if d.strip():
            print(f'{pid}\tNOT RESTORED: {d}', flush=True)
            sys.exit(1)
