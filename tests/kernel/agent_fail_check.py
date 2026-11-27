#!/usr/bin/env python3
"""The agent rig's check of its failure course (afail; M2.7c, #76):
`skyzfs-agent run` on its default schedule on a volume that fails while
it is served (tests/kernel/agent-record.sh, rec_afail, says what each
case does and what it leaves).

Usage (agent_check.py hands an afail recording on to it):
  agent_fail_check.py NAME DIR

Each line of results.tsv is KEY, VERDICT, [what it rests on] and the
detail, as agent_check.py's. Exit 1 is a verdict against the agent or
the daemon, 2 the rig's own error, 0 neither.

Both forms of the round and lost lines are read: zfs-topology's, and
M2.7b-2's (#179: early_ms= and reopen=held on the round line, step=held
on the lost line), so that either pull request can merge first.

Case U (lost with no pool imported, then no client, then a new
attachment):
- premise_U_round_1 (rig): round 1's line came before the failure, with
  the pool imported then.
- premise_U_failed_before_tick (rig): the failure came STEP_WITHIN s or
  more before round 2's tick (round 1's tick and 60 s), so that a step
  within STEP_WITHIN s is the note's doing and not the tick's. Where
  not, pin_U_lost_line and pin_U_step_within are not judged (INFO): the
  rig's lateness is not the agent's (D3 of #180's review).
- pin_U_daemon_failed (server): the daemon ended with 3 on the failure,
  and its note was there after it, written after the signal.
- pin_U_lost_line (server): one `lost` line, the next round's: note=whole
  reason=image-failed held=unknown (nothing held the device, and the
  daemon's exclusive open proves no holder) step=not-imported reopen=-
  state=- looked_ms=- verdict=failed, for the pool; that round ok, it
  begins nothing, its words on standard error, and the run goes on
  (round 3 is there).
- pin_U_step_within (server): the lost line came within STEP_WITHIN s
  of the note's writing: the note is watched about every second
  (NOTE_EVERY) and ends the wait for the tick, so the step runs within
  seconds and not at the next tick (the lead's ruling, F1 of #173's
  review). obs_U_times gives the times.
- pin_U_refused (server): the start without --acknowledge-failure
  ended with 4, and the note was still there after it.
- pin_U_unheld (server): the round after it says the volume failed with
  no client holding its device, prints no `lost` line, is ok and begins
  nothing.
- pin_U_moved (server): the run ended with 12 after the next round's
  line, the last it printed, saying why, within END_WITHIN s of it.
- pin_U_default_schedule (server): with no --every-s, the rounds after
  the early one keep the ticks of 60 s from the first (EVERY_DEFAULT),
  give or take their late_ms and TICK_SLACK; the early round shifts none
  of them. Round k begins (k - 1) x 60 s after round 1 where round 2
  carries no early_ms (zfs-topology before #179: the early round takes
  the next tick) and (k - 2) x 60 s where it does (#179: it keeps its
  tick), and then its early_ms is how long before its tick it began.
  Rounds 3 and 4 carry no early_ms. #179's quick retry follows only a
  retry verdict, which U never gives.
- premise_U_refused_in_time, premise_U_served_in_time (rig): the
  refused start ended before round 3 began, and the acknowledged start
  served and its pool was imported before round 4 began.

Case W (lost while a publish waits for its cut, the device held):
- premise_W_waiting (rig): the run reached asked:after before the
  failure and had printed nothing by then: its first round was waiting
  for its cut.
- obs_W_in_flight: the trace's last point before the failure, how many
  assist syncs had begun, and whether one was in flight at the failure
  (its `assist before` with no `after` yet) and when it returned: a ZFS
  command in flight is not cut by the agent.
- pin_W_daemon_failed (server): as U's.
- pin_W_lost_line (server): after round 1's line, one `lost` line and
  then the round line of its round, the last two lines: note=whole
  reason=image-failed held=yes state=SUSPENDED verdict=lost, step
  reopened (reopen ok or failed) or suspended (zed may suspend the pool
  first); a reopened step looked for at most LOOK_LIMIT_MS; its round
  begins nothing.
- pin_W_step_within (server): the lost line came within W_STEP_WITHIN s
  of the note's writing: the wait for the cut ends on the note, and the
  step (a reopen looks for at most 10 s) is not let run to its limit
  (G1 of #180's review; 0.20 to 2.17 s from the failure to the end in
  the first ten jobs).
- pin_W_exit (server): the run ended with 11 within END_WITHIN s of its
  last line.
- pin_W_suspended (kernel): the pool's kstat state read SUSPENDED after
  the run.
- pin_W_again (server): `run` again in the same directory ended with 11,
  its first round's `lost` line step=suspended reopen=- state=SUSPENDED
  looked_ms=- verdict=lost, then that round's line, the last two lines,
  within END_WITHIN s.
- obs_W_times: the failure to the note, the note to round 1's line (the
  wait ended), round 1's line to the lost line (the step), looked_ms,
  and the failure to the end; obs_W_again_times the second run's.

And the course's:
- premise_zed (rig): zed runs where the workflow asked for it, and not
  otherwise (zed.tsv); obs_zed its lines in zed.log.
- pin_watcher_reads (kernel and server): the watcher beside the daemon
  (cut_watch.py, node B's side of the uploads, stopped at the course's
  end) wrote no VERDICT line to watch.log: node B read the content of
  every cut it found at head, the cuts uploaded before each failure
  included. A watch.log with no line or with an ERROR line is the rig's
  error (premise_watcher). B1 of #180's review: agent_check.py's own
  pin is not reached for afail.
- pin_ID_PIN (server) for each verdict the recorder wrote
  (cases/ID/verdicts.tsv); rig_error_in_the_course for DIR/rig-error.tsv;
  premise_ID_rig for a case's rig-error.txt (that case not judged).
"""
import os
import re
import sys
import time

COURSES = ('afail',)
POOL = 'skzcut'
EVERY_DEFAULT = 60
STEP_WITHIN = 3.0
W_STEP_WITHIN = 5.0
END_WITHIN = 2.0
TICK_SLACK = 1.0
LOOK_LIMIT_MS = 10_500
# What a command on a suspended pool says (zed's, zpool's), counted where
# it was seen.
SUSPENDED_SAID = 'pool I/O is currently suspended'
REASON = 'image-failed'

ROUND_LINE = re.compile(r'round ([1-9][0-9]*) (ok|failed) took_ms=([0-9]+) late_ms=([0-9]+) skipped=([0-9]+) '
                        r'begun=(\S+) under_way=([0-9]+)((?: [a-z][a-z_]*=[^ =]+)*)')
LOST_LINE = re.compile(r'lost (\S+) volume=([0-9a-f]+) note=(whole|torn) reason=(\S+) held=(yes|unknown) '
                       r'step=(not-imported|suspended|reopened|held|failed) reopen=(ok|failed|-) state=(\S+) '
                       r'looked_ms=([0-9]+|-) verdict=(lost|failed|retry)')
# The two lines as zfs-topology has them and as M2.7b-2 (#179) adds to
# them, read both ways so that either pull request can merge first (the
# lead's ruling on #180): the lost line's step=held (the cap on reopens,
# never reached here: it needs ten reopens of one note), and the round
# line's early_ms=<ms> (a round begun before its tick, which with #179
# keeps its tick: round_ticks) and reopen=held, among its trailing
# name=value fields.
ROUND_EXTRA = {'early_ms': re.compile(r'[0-9]+'), 'reopen': re.compile(r'held')}
LOST_FIELDS = ('pool', 'volume', 'note', 'reason', 'held', 'step', 'reopen', 'state', 'looked_ms', 'verdict')
# The outcome lines `run` prints among its rounds (agent_check.py's
# read_outputs).
OUTCOME = re.compile(r'snap/\S+ \S+|abandoned \S+ \S+|stopped \S+ .+|unsettled \S+')


class RigError(Exception):
    pass


def flat(text):
    return ' '.join(str(text).split())


def read_stamps(text, what):
    """LABEL.out.tsv or .err.tsv -> [seconds]: the arrival of each line,
    numbered from 1."""
    out = []
    for k, ln in enumerate(text.splitlines(), 1):
        f = ln.split('\t')
        try:
            ok = len(f) == 2 and int(f[0]) == k
            at = float(f[1]) if ok else 0.0
        except ValueError:
            ok = False
        if not ok:
            raise RigError(f'{what}: line {k} is not {k}, seconds: {ln!r}')
        out.append(at)
    return out


def read_run(out, out_tsv, err, err_tsv):
    """A run's standard output and errors with their arrivals -> {'lines':
    [{'kind': round|lost|outcome, 'f': fields, 'text', 'at'}], 'errs':
    [(text, at)]}. A line of output that is none of the three is the
    rig's error; a line not yet stamped has `at` None."""
    stamps = read_stamps(out_tsv, 'out.tsv')
    lines = []
    for k, ln in enumerate(out.splitlines()):
        at = stamps[k] if k < len(stamps) else None
        r = ROUND_LINE.fullmatch(ln)
        m = LOST_LINE.fullmatch(ln)
        if r:
            f = {'n': int(r[1]), 'ok': r[2] == 'ok', 'took_ms': int(r[3]), 'late_ms': int(r[4]),
                 'skipped': int(r[5]), 'begun': None if r[6] == '-' else r[6], 'under_way': int(r[7])}
            f.update(round_extra(r[8], ln))
            lines.append({'kind': 'round', 'f': f, 'text': ln, 'at': at})
        elif m:
            lines.append({'kind': 'lost', 'f': dict(zip(LOST_FIELDS, m.groups())), 'text': ln, 'at': at})
        elif OUTCOME.fullmatch(ln):
            lines.append({'kind': 'outcome', 'f': {}, 'text': ln, 'at': at})
        else:
            raise RigError(f"a line of the run's output that is not a round, lost or outcome line: {ln!r}")
    estamps = read_stamps(err_tsv, 'err.tsv')
    errs = [(ln, estamps[k] if k < len(estamps) else None) for k, ln in enumerate(err.splitlines())]
    return {'lines': lines, 'errs': errs}


def round_extra(tail, ln):
    """A round line's trailing name=value fields -> {'early_ms': int or
    None, 'reopen': 'held' or None}. A name twice, or early_ms or reopen
    not as ROUND_EXTRA has them, is the rig's error."""
    seen = {}
    for w in tail.split():
        k, v = w.split('=', 1)
        if k in seen:
            raise RigError(f'a round line with {k} twice: {ln!r}')
        seen[k] = v
    for k, want in ROUND_EXTRA.items():
        if k in seen and not want.fullmatch(seen[k]):
            raise RigError(f'a round line with {k}={seen[k]}: {ln!r}')
    return {'early_ms': int(seen['early_ms']) if 'early_ms' in seen else None, 'reopen': seen.get('reopen')}


def round_ticks(run, n):
    """The ticks of 60 s from round 1's to round N's: one for each round
    from 2 to N that carries no early_ms. Before #179 the early round
    carries none and takes the next tick, whose round does not come; with
    #179 it says early_ms and keeps it (D1 of #173's review), so round 3
    comes on that tick."""
    return sum(1 for r in rounds(run) if 2 <= r['f']['n'] <= n and r['f']['early_ms'] is None)


def rounds(run):
    return [ln for ln in run['lines'] if ln['kind'] == 'round']


def round_of(run, n):
    return next((ln for ln in rounds(run) if ln['f']['n'] == n), None)


def index(run, line):
    """The place of LINE (that very line) among the run's lines."""
    return next(k for k, ln in enumerate(run['lines']) if ln is line)


def began(line):
    """A round line -> its round's beginning on the rig's clock (its
    arrival less its took_ms), or None where it was not stamped."""
    return None if line is None or line['at'] is None else line['at'] - line['f']['took_ms'] / 1000


def read_fail_at(text):
    f = text.strip().split('\t')
    try:
        if len(f) != 2:
            raise ValueError
        return float(f[0]), int(f[1])
    except ValueError:
        raise RigError(f'fail-at: not seconds, ns: {text!r}') from None


def read_notes(text):
    """note.tsv -> {label: (mtime or None, size or None)}."""
    out = {}
    for ln in text.splitlines():
        f = ln.split('\t')
        try:
            if len(f) != 3:
                raise ValueError
            out[f[0]] = (None, None) if f[1] == 'none' else (float(f[1]), int(f[2]))
        except ValueError:
            raise RigError(f'note.tsv: not label, mtime, size: {ln!r}') from None
    return out


def read_time(text, what):
    try:
        return float(text.strip())
    except ValueError:
        raise RigError(f'{what}: not a time: {text!r}') from None


def lost_wrong(f, want):
    """A lost line's fields and the {field: value or (values,)} wanted ->
    [what differs]."""
    bad = []
    for k, v in want.items():
        ok = f[k] in v if isinstance(v, tuple) else f[k] == v
        if not ok:
            bad.append(f'{k}={f[k]}, not {"|".join(v) if isinstance(v, tuple) else v}')
    return bad


def ms(a, b):
    return '?' if a is None or b is None else f'{(b - a) * 1000:.0f}'


def judge_failed(cid, failed_rc, notes, fail_real):
    """The daemon's end on the failure and its note -> [what is not so]."""
    bad = []
    if failed_rc != '3':
        bad.append(f'the daemon ended with {failed_rc} on the failure, not 3')
    mtime, size = notes.get('failed', (None, None))
    if mtime is None:
        bad.append('no failure note after the daemon ended')
    elif mtime < fail_real - 0.05:
        bad.append(f'the note was written {fail_real - mtime:.3f} s before the failure: not this failure\'s')
    return bad


def judge_unknown(c, j):
    """Case U. C: {'run', 'rc', 'fail': (real, mono), 'failed_rc', 'notes',
    'refused_rc', 'refused_at', 'served_at', 'ended_at'}; J: the judge."""
    run = c['run']
    fail_real = c['fail'][0]
    r1 = round_of(run, 1)
    j.premise('premise_U_round_1',
              [] if r1 and r1['at'] is not None and r1['at'] < fail_real
              else ['round 1\'s line did not come before the failure'],
              'round 1 came before the failure, with the pool imported then')
    j.holds('pin_U_daemon_failed', judge_failed('U', c['failed_rc'], c['notes'], fail_real), 'server',
            'the daemon ended with 3, its note written after the signal')
    b1 = began(r1)
    tick2 = None if b1 is None else b1 - r1['f']['late_ms'] / 1000 + EVERY_DEFAULT
    in_time = j.premise(
        'premise_U_failed_before_tick',
        [] if tick2 is not None and fail_real + STEP_WITHIN <= tick2 else
        ['no round 1 to time round 2\'s tick from' if tick2 is None else
         f'the volume failed {tick2 - fail_real:.2f} s before round 2\'s tick, not {STEP_WITHIN} s or more: '
         'a step within the limit could be the tick\'s'],
        f'the volume failed {tick2 - fail_real:.1f} s before round 2\'s tick' if tick2 is not None else '')
    lost = [ln for ln in run['lines'] if ln['kind'] == 'lost']
    lines = run['lines']
    bad = []
    k = next((i for i, ln in enumerate(lines) if ln['kind'] == 'lost'), None)
    r2 = round_of(run, 2)
    if len(lost) != 1:
        bad.append(f'{len(lost)} lost lines, not 1')
    if k is None:
        bad.append('no lost line')
    else:
        f = lost[0]['f']
        bad += lost_wrong(f, {'pool': POOL, 'note': 'whole', 'reason': REASON, 'held': 'unknown',
                              'step': 'not-imported', 'reopen': '-', 'state': '-', 'looked_ms': '-',
                              'verdict': 'failed'})
        nxt = lines[k + 1] if k + 1 < len(lines) else None
        if nxt is None or nxt['kind'] != 'round' or nxt['f']['n'] != 2:
            bad.append(f"the line after it is {nxt['text'] if nxt else 'none'!r}, not round 2's")
    if r2 is not None:
        if not r2['f']['ok']:
            bad.append('round 2 failed')
        if r2['f']['begun'] is not None:
            bad.append(f"round 2 began {r2['f']['begun']}")
    said = ('round 2: the volume failed in this boot (note=whole reason=image-failed held=unknown), and the '
            'pool is not imported: nothing is suspended; run goes on')
    if not any(said in e for e, _ in run['errs']):
        bad.append('no line on standard error that round 2 found the pool not imported and goes on')
    if round_of(run, 3) is None:
        bad.append('no round 3: the run did not go on')
    if in_time:
        j.holds('pin_U_lost_line', bad, 'server',
                'one lost line, held=unknown step=not-imported verdict=failed; round 2 ok, nothing begun; the run '
                'went on')
    else:
        j.info('pin_U_lost_line', 'rig', 'not judged: the failure came too near round 2\'s tick')
    note = c['notes'].get('failed', (None, None))[0]
    at = lost[0]['at'] if lost else None
    j.info('obs_U_times', 'server', f'failure to note {ms(fail_real, note)} ms, note to lost line {ms(note, at)} ms, '
           f'failure to lost line {ms(fail_real, at)} ms; round 2 took {r2["f"]["took_ms"] if r2 else "?"} ms')
    if not in_time:
        j.info('pin_U_step_within', 'rig', 'not judged: the failure came too near round 2\'s tick')
    elif note is None or at is None:
        j.holds('pin_U_step_within', ['no note or no lost line to time'], 'server', '')
    else:
        j.holds('pin_U_step_within',
                [] if at - note <= STEP_WITHIN else
                [f'the lost line came {at - note:.2f} s after the note, not within {STEP_WITHIN} s'],
                'server', f'the lost line came {at - note:.2f} s after the note')
    bad = []
    if c['refused_rc'] != '4':
        bad.append(f'the start without --acknowledge-failure ended with {c["refused_rc"]}, not 4')
    if c['notes'].get('refused', (None, None))[0] is None:
        bad.append('no failure note after the refused start')
    j.holds('pin_U_refused', bad, 'server', 'refused with 4, the note kept')
    r3 = round_of(run, 3)
    r4 = round_of(run, 4)
    j.premise('premise_U_refused_in_time',
              [] if r3 and began(r3) is not None and c['refused_at'] < began(r3)
              else ['the refused start did not end before round 3 began'],
              f'the refused start ended {began(r3) - c["refused_at"]:.1f} s before round 3 began' if r3 and began(r3)
              else '')
    bad = []
    if r3 is None:
        bad.append('no round 3')
    else:
        k3 = index(run, r3)
        k2 = index(run, r2) if r2 is not None else -1
        if any(ln['kind'] == 'lost' for ln in lines[k2 + 1:k3]):
            bad.append('a lost line in round 3')
        if not r3['f']['ok']:
            bad.append('round 3 failed')
        if r3['f']['begun'] is not None:
            bad.append(f"round 3 began {r3['f']['begun']}")
        said = 'round 3: the volume failed in this boot ('
        tail = 'and no client held its device: no pool is touched'
        if not any(said in e and tail in e for e, _ in run['errs']):
            bad.append('no line on standard error that round 3 found no client held the device')
    j.holds('pin_U_unheld', bad, 'server', 'round 3: no client held the device, no pool touched, ok')
    j.premise('premise_U_served_in_time',
              [] if r4 and began(r4) is not None and c['served_at'] < began(r4)
              else ['the acknowledged start and the import were not done before round 4 began'],
              f'served and imported {began(r4) - c["served_at"]:.1f} s before round 4 began' if r4 and began(r4)
              else '')
    bad = []
    if c['rc'] != '12':
        bad.append(f'the run ended with {c["rc"]}, not 12')
    if r4 is None:
        bad.append('no round 4')
    elif lines[-1] is not r4:
        bad.append(f"its last line is {lines[-1]['text']!r}, not round 4's")
    else:
        if not any('round 4: ' in e and 'run ends, to be started again on the device as it is now' in e
                   for e, _ in run['errs']):
            bad.append('no line on standard error that round 4 saw the device or its attachment change')
        if r4['at'] is None or c['ended_at'] is None or c['ended_at'] - r4['at'] > END_WITHIN:
            bad.append(f'it ended {ms(r4["at"], c["ended_at"])} ms after round 4\'s line, not within '
                       f'{END_WITHIN} s')
    j.holds('pin_U_moved', bad, 'server', 'ended with 12 after round 4, the new attachment said')
    # #179's quick retry follows only a retry verdict, which U never
    # gives (not-imported is failed, and so is the round after it), so
    # no round of U is a quick one (the lead's ruling on #180).
    bad = []
    if r2 is not None and r2['f']['early_ms'] is not None and b1 is not None and began(r2) is not None:
        early = tick2 - began(r2)
        if abs(r2['f']['early_ms'] / 1000 - early) > TICK_SLACK:
            bad.append(f"round 2 says early_ms={r2['f']['early_ms']}, but it began {early:.2f} s before its tick")
    for r in (r3, r4):
        if r is None or b1 is None or began(r) is None:
            continue
        if r['f']['early_ms'] is not None:
            bad.append(f"round {r['f']['n']} says early_ms={r['f']['early_ms']}: it did not come on its tick")
            continue
        want = round_ticks(run, r['f']['n']) * EVERY_DEFAULT + (r['f']['late_ms'] - r1['f']['late_ms']) / 1000
        got = began(r) - b1
        if abs(got - want) > TICK_SLACK:
            bad.append(f"round {r['f']['n']} began {got:.2f} s after round 1, not {want:.2f} s")
    if r3 is None or r4 is None or b1 is None:
        bad.append('rounds 1, 3 and 4 are not all there to time')
    j.holds('pin_U_default_schedule', bad, 'server',
            f'rounds 3 and 4 began {began(r3) - b1:.2f} s and {began(r4) - b1:.2f} s after round 1; '
            + (f"round 2 early_ms={r2['f']['early_ms']}, its tick kept" if r2 and r2['f']['early_ms'] is not None
               else 'round 2 with no early_ms, its tick taken')
            if not bad else '')


def judge_trace(trace, fail_mono):
    """W's trace (ns, effect, side, name) and the failure's CLOCK_MONOTONIC
    ns -> (whether asked:after came before it, detail of what was in
    flight)."""
    before = [t for t in trace if t[0] <= fail_mono]
    asked = any(t[1] == 'asked' and t[2] == 'after' for t in before)
    assists = [t for t in before if t[1] == 'assist' and t[2] == 'before']
    last = before[-1] if before else None
    flight = last is not None and last[1] == 'assist' and last[2] == 'before'
    ret = next((t for t in trace if t[0] > fail_mono and t[1] == 'assist' and t[2] == 'after'), None)
    after = [t for t in trace if t[0] > fail_mono]
    detail = (f"last point before the failure: {last[1] + ' ' + last[2] if last else 'none'}"
              f"{f' ({(fail_mono - last[0]) / 1e6:.0f} ms before it)' if last else ''}; {len(assists)} assist syncs "
              f"begun before it; one in flight at it: {'yes' if flight else 'no'}")
    if flight:
        detail += (f", returned {(ret[0] - fail_mono) / 1e6:.0f} ms after the failure" if ret
                   else ', never returned in the trace')
    detail += f"; points after the failure: {', '.join(t[1] + ' ' + t[2] for t in after) or 'none'}"
    return asked, detail


def read_trace(text):
    out = []
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 4 or not f[0].isdigit() or f[2] not in ('before', 'after'):
            raise RigError(f'a trace line that is not ns, effect, side, name: {ln!r}')
        out.append((int(f[0]), f[1], f[2], f[3]))
    return out


def judge_lost_end(run, rc, ended_at, step):
    """A run that must end with 11 on its lost line -> ([what is not so of
    its lines], [what is not so of its end], the lost line or None).
    STEP: the step(s) allowed."""
    lines = run['lines']
    bad = []
    end = []
    lost = [ln for ln in lines if ln['kind'] == 'lost']
    if rc != '11':
        end.append(f'the run ended with {rc}, not 11')
    if len(lost) != 1:
        bad.append(f'{len(lost)} lost lines, not 1')
    if len(lines) < 2 or lines[-2]['kind'] != 'lost' or lines[-1]['kind'] != 'round':
        bad.append('its last two lines are not a lost line and its round line')
        return bad, end, (lost[0] if lost else None)
    lo, rd = lines[-2], lines[-1]
    bad += lost_wrong(lo['f'], {'pool': POOL, 'note': 'whole', 'reason': REASON, 'held': 'yes',
                                'state': 'SUSPENDED', 'verdict': 'lost', 'step': step})
    if lo['f']['step'] == 'suspended':
        bad += lost_wrong(lo['f'], {'reopen': '-', 'looked_ms': '-'})
    elif lo['f']['step'] == 'reopened':
        bad += lost_wrong(lo['f'], {'reopen': ('ok', 'failed')})
        if lo['f']['looked_ms'] == '-' or int(lo['f']['looked_ms']) > LOOK_LIMIT_MS:
            bad.append(f"looked_ms={lo['f']['looked_ms']}, not within {LOOK_LIMIT_MS}")
    if rd['f']['begun'] is not None:
        bad.append(f"the lost round began {rd['f']['begun']}")
    if rd['at'] is None or ended_at is None or ended_at - rd['at'] > END_WITHIN:
        end.append(f'it ended {ms(rd["at"], ended_at)} ms after its last line, not within {END_WITHIN} s')
    return bad, end, lo


def reopen_ms(lo, at1, atl):
    """The reopen's time, from the stamps: the step (round 1's line to the
    lost line) less looked_ms, for a reopened step."""
    if lo is None or lo['f']['step'] != 'reopened' or at1 is None or atl is None \
            or not lo['f']['looked_ms'].isdigit():
        return ''
    return f", the reopen about {(atl - at1) * 1000 - int(lo['f']['looked_ms']):.0f} ms (the step less looked_ms)"


def judge_wait(c, j):
    """Case W. C: {'run', 'rc', 'ended_at', 'started_at', 'fail',
    'failed_rc', 'notes', 'trace', 'state', 'again': {'run', 'rc',
    'ended_at', 'started_at'}, 'state_again'}."""
    run = c['run']
    fail_real, fail_mono = c['fail']
    asked, flight = judge_trace(c['trace'], fail_mono)
    early = [ln for ln in run['lines'] if ln['at'] is not None and ln['at'] <= fail_real]
    bad = [] if asked else ['the trace did not reach asked:after before the failure']
    if early:
        bad.append(f"the run printed {early[0]['text']!r} before the failure")
    j.premise('premise_W_waiting', bad, 'the publish was asked and its round was waiting at the failure')
    j.info('obs_W_in_flight', 'server', flight)
    j.holds('pin_W_daemon_failed', judge_failed('W', c['failed_rc'], c['notes'], fail_real), 'server',
            'the daemon ended with 3, its note written after the signal')
    bad, end, lo = judge_lost_end(run, c['rc'], c['ended_at'], ('reopened', 'suspended'))
    r1 = round_of(run, 1)
    if r1 is None or lo is None or index(run, r1) > index(run, lo):
        bad.append('no round 1 line before the lost line: the wait did not end on the note')
    j.holds('pin_W_lost_line', bad, 'server',
            f"round 1, then lost with step={lo['f']['step']} reopen={lo['f']['reopen']} "
            f"looked_ms={lo['f']['looked_ms']}" if lo else '')
    note = c['notes'].get('failed', (None, None))[0]
    at1 = r1['at'] if r1 else None
    atl = lo['at'] if lo else None
    if note is None or atl is None:
        j.holds('pin_W_step_within', ['no note or no lost line to time'], 'server', '')
    else:
        j.holds('pin_W_step_within',
                [] if atl - note <= W_STEP_WITHIN else
                [f'the lost line came {atl - note:.2f} s after the note, not within {W_STEP_WITHIN} s'],
                'server', f'the lost line came {atl - note:.2f} s after the note')
    j.holds('pin_W_exit', end, 'server', 'ended with 11 on its lost line')
    j.info('obs_W_times', 'server',
           f'failure to note {ms(fail_real, note)} ms, note to round 1\'s line {ms(note, at1)} ms (the wait '
           f'ended), round 1\'s line to the lost line {ms(at1, atl)} ms (the step), looked_ms '
           f"{lo['f']['looked_ms'] if lo else '?'}{reopen_ms(lo, at1, atl)}, failure to the end "
           f"{ms(fail_real, c['ended_at'])} ms")
    st = c['state'].strip()
    j.holds('pin_W_suspended', [] if st == 'SUSPENDED' else [f'the pool\'s state was {st!r}, not SUSPENDED'],
            'kernel', 'the pool read SUSPENDED after the run')
    a = c['again']
    bad, end, _ = judge_lost_end(a['run'], a['rc'], a['ended_at'], 'suspended')
    if [r['f']['n'] for r in rounds(a['run'])] != [1]:
        bad.append(f"its rounds are {[r['f']['n'] for r in rounds(a['run'])]}, not round 1 alone")
    j.holds('pin_W_again', bad + end, 'server', 'run again: 11 at its first round, step=suspended, no reopen')
    j.info('obs_W_again_times', 'server', f"start to end {ms(a['started_at'], a['ended_at'])} ms; the state after it "
           f"{c['state_again'].strip()!r}")


def judge_zed(text, j):
    f = dict(ln.split('\t', 1) for ln in text.splitlines() if '\t' in ln)
    want, runs = f.get('want'), f.get('runs')
    j.premise('premise_zed', [] if want in ('on', 'off') and want == runs
              else [f'zed runs: {runs}, the workflow asked for {want}'], f'zed {runs}, as asked')


class Judge:
    """The verdict lines and the status they make."""

    def __init__(self, name, out=sys.stdout):
        self.name = name
        self.lines = []
        self.rc = 0
        self.failed = False
        self.out = out

    def put(self, key, verdict, rests, detail):
        self.lines.append(f'{key}\t{verdict}\t{flat(f"[{rests}] {detail}")}')
        print(f'{self.name}: {key}: {verdict}: [{rests}] {flat(detail)}', file=self.out, flush=True)

    def holds(self, key, bad, rests, good):
        if bad:
            self.failed = True
            self.put(key, 'FAILED', rests, '; '.join(bad))
        else:
            self.put(key, 'ok', rests, good)

    def premise(self, key, bad, good):
        if bad:
            self.rc = max(self.rc, 2)
            self.put(key, 'ERROR', 'rig', '; '.join(bad))
        else:
            self.put(key, 'ok', 'rig', good)
        return not bad

    def info(self, key, rests, detail):
        self.put(key, 'INFO', rests, detail)

    def status(self):
        return 1 if self.failed else self.rc


def judge_course(rdir, j):
    def text_of(path, need=True):
        p = os.path.join(rdir, path)
        if not os.path.exists(p):
            if need:
                raise RigError(f'no {path}')
            return ''
        with open(p, errors='replace') as f:
            return f.read()

    cdir = os.path.join(rdir, 'cases')
    for cid in sorted(os.listdir(cdir)) if os.path.isdir(cdir) else []:
        for ln in text_of(os.path.join('cases', cid, 'verdicts.tsv'), need=False).splitlines():
            pin, _, what = ln.partition('\t')
            if not re.fullmatch(r'[a-z][a-z_]*', pin) or not what:
                j.premise(f'premise_{cid}_verdicts', [f'cases/{cid}/verdicts.tsv: not pin, what: {ln!r}'], '')
                continue
            j.holds(f'pin_{cid}_{pin}', [what], 'server', '')
    for ln in text_of('rig-error.tsv', need=False).splitlines():
        c, _, m = ln.partition('\t')
        j.rc = max(j.rc, 2)
        j.put('rig_error_in_the_course', 'ERROR', 'rig', f'in case {c}: {m}; the cases before it are judged')
    judge_zed(text_of('zed.tsv'), j)
    zlog = text_of('zed.log', need=False)
    j.info('obs_zed', 'kernel', f'{len(zlog.splitlines())} lines of zed\'s journal over the course')
    said = [(what, sum(SUSPENDED_SAID in ln for ln in text_of(path, need=False).splitlines()))
            for what, path in (('zed\'s journal', 'zed.log'), ('U\'s run', 'cases/U/publish.err'),
                               ('W\'s run', 'cases/W/publish.err'), ('W\'s second run', 'cases/W/again.err'),
                               ('zpool status', 'cases/W/zpool-status.txt'))]
    j.info('obs_suspended_said', 'kernel', f"lines with '{SUSPENDED_SAID}': "
           + ', '.join(f'{what} {n}' for what, n in said))

    def run_of(cid, label):
        d = os.path.join('cases', cid)
        rc = text_of(os.path.join(d, f'{label}.rc')).strip()
        ended = text_of(os.path.join(d, f'{label}.ended-at'), need=False)
        return {'run': read_run(text_of(os.path.join(d, f'{label}.out')),
                                text_of(os.path.join(d, f'{label}.out.tsv'), need=False),
                                text_of(os.path.join(d, f'{label}.err')),
                                text_of(os.path.join(d, f'{label}.err.tsv'), need=False)),
                'rc': rc, 'ended_at': read_time(ended, f'{label}.ended-at') if ended.strip() else None,
                'started_at': read_time(text_of(os.path.join(d, f'{label}.started-at')), f'{label}.started-at')}

    cases = [ln.split('\t') for ln in text_of('cases.tsv').splitlines() if ln]
    done = {c[0] for c in cases}
    for cid, judge in (('U', judge_unknown), ('W', judge_wait)):
        d = os.path.join('cases', cid)
        cerr = text_of(os.path.join(d, 'rig-error.txt'), need=False).strip()
        if cerr:
            j.premise(f'premise_{cid}_rig', [f"the rig's own error in the case: {cerr}"], '')
            continue
        if not os.path.isdir(os.path.join(rdir, d)):
            j.put(f'case_{cid}', 'INFO', 'rig', 'not reached')
            continue
        if cid not in done:
            j.put(f'case_{cid}', 'INFO', 'rig', 'not ended (no line in cases.tsv): judged from what it left')
        try:
            c = run_of(cid, 'publish')
            c['fail'] = read_fail_at(text_of(os.path.join(d, 'fail-at')))
            c['failed_rc'] = text_of(os.path.join(d, 'failed.rc')).strip()
            c['notes'] = read_notes(text_of(os.path.join(d, 'note.tsv')))
            if cid == 'U':
                c['refused_rc'] = text_of(os.path.join(d, 'refused.rc')).strip()
                c['refused_at'] = read_time(text_of(os.path.join(d, 'refused-at')), 'refused-at')
                c['served_at'] = read_time(text_of(os.path.join(d, 'served-at')), 'served-at')
            else:
                c['trace'] = read_trace(text_of(os.path.join(d, 'trace.tsv')))
                c['state'] = text_of(os.path.join(d, 'pool-state.txt'))
                c['state_again'] = text_of(os.path.join(d, 'pool-state-again.txt'), need=False)
                c['again'] = run_of(cid, 'again')
        except RigError as e:
            j.premise(f'premise_{cid}_files', [str(e)], '')
            continue
        judge(c, j)
    judge_watcher(text_of('watch.log', need=False), j)


def judge_watcher(text, j):
    """watch.log's lines: no line or an ERROR is the rig's error, a
    VERDICT a verdict (node B could not read head's content)."""
    watch = text.splitlines()
    err = [ln.split('\t', 1)[-1] for ln in watch if '\tERROR ' in ln]
    if j.premise('premise_watcher',
                 ['the watcher wrote no line to watch.log'] if not watch else
                 [f'the watcher: {err[0]}'] if err else [],
                 f'{len(watch)} lines in watch.log'):
        bad = [ln.split('\tVERDICT ', 1)[1] for ln in watch if '\tVERDICT ' in ln]
        j.holds('pin_watcher_reads', bad, 'kernel and server',
                f'node B read the content of every cut the watcher found at head ({len(watch)} lines)')


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 2:
        sys.exit(__doc__)
    name, rdir = argv
    t0 = time.monotonic()
    j = Judge(name)
    try:
        if name not in COURSES:
            raise RigError(f'not the failure course: {name}')
        judge_course(rdir, j)
    except RigError as e:
        j.rc = 2
        j.put('rig_error', 'ERROR', 'rig', str(e))
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        j.rc = 2
        j.put('rig_error', 'ERROR', 'rig', f'{type(e).__name__}: {e}')
    j.put('time', 'INFO', 'rig', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in j.lines))
    return j.status()


if __name__ == '__main__':
    sys.exit(main())
