"""Where a request's time goes in skyzfsd, from two readings of `status`.

The benchmark reads the daemon's status before and after a measured
window. Since status version 5 it carries `time_*` keys: counts and
nanoseconds that only grow, the longest of each (`*_max_ns`, since the
daemon started serving) and what the longest wait was for
(`*_longest_wait_cause*`). `delta` takes the difference of the two
readings; `render` prints the tables. Nothing here judges: the numbers
are observations, and the identities are a guard on the reading.

A reading is a dict of the status's lines, key -> text.
"""

KINDS = ('read', 'write', 'flush', 'discard', 'zeroes')
# What the one worker does, in the status's order.
ACTIVITIES = ('idle', 'read', 'write', 'flush', 'discard', 'zeroes', 'step_walk', 'step_save', 'step_apply',
              'step_home_sync', 'step_log_start', 'step_end', 'own_sync', 'other')
STEP_PARTS = ('walk', 'save', 'apply', 'home_sync', 'log_start', 'end')


def is_end_value(key):
    """A key whose value is not a sum: the longest so far, or its cause."""
    return key.endswith('_max_ns') or 'longest_wait_cause' in key


def delta(before, after, before_at_ns=None, after_at_ns=None):
    """-> {'ok', 'why', 'delta', 'end', 'before', 'after', 'window_ns'}.

    `delta` has the difference of every time_* key that is a sum or a
    count, `end` the end reading's value of the others. A reading
    without time_* keys (a status older than version 5, or one that
    could not be read) gives ok False and says why; so does a pid that
    changed or a value that went down, since the sums are since the
    daemon started.
    """
    tb = {k: v for k, v in before.items() if k.startswith('time_')}
    ta = {k: v for k, v in after.items() if k.startswith('time_')}
    out = {'ok': False, 'why': '', 'delta': {}, 'end': {}, 'before': tb, 'after': ta, 'window_ns': None}
    if before_at_ns is not None and after_at_ns is not None:
        out['window_ns'] = after_at_ns - before_at_ns
    if not tb or not ta:
        which = ' and '.join(n for n, t in (('before the window', tb), ('after it', ta)) if not t)
        out['why'] = (f'the status read {which} has no time_* keys: the reading failed, '
                      'or the status is older than version 5')
        return out
    why = []
    if before.get('pid') != after.get('pid'):
        why.append(f"the pid changed ({before.get('pid')} then {after.get('pid')}): the sums are not of one daemon")
    missing = sorted(set(tb) ^ set(ta))
    if missing:
        why.append(f"{len(missing)} keys are in one reading only: {' '.join(missing[:5])}")
    for key, value in ta.items():
        if is_end_value(key):
            out['end'][key] = int(value) if value.isdigit() else value
            continue
        if key not in tb:
            continue
        if not (value.isdigit() and tb[key].isdigit()):
            why.append(f'{key} is not a number ({tb[key]!r} then {value!r})')
            continue
        d = int(value) - int(tb[key])
        if d < 0:
            why.append(f'{key} went down ({tb[key]} then {value})')
        out['delta'][key] = d
    out['why'] = '; '.join(why)
    out['ok'] = not why
    return out


def _per(total, count, unit=1000.0):
    """total ns over count things, in µs (unit 1000) or ms (1e6); None without a count."""
    return None if not count or total is None else total / count / unit


def _f(v, fmt='.1f'):
    return 'n/a' if v is None else format(v, fmt)


def _share(part, whole):
    return 'n/a' if not whole or part is None else f'{100.0 * part / whole:.1f}%'


def life(d, kind):
    """A request's life in µs, as means over the window: [(stage, µs or None)], and its count.

    The pump's submit time is over every kind (the status does not
    split it); ready->taken and taken->committed are split between
    reads and everything else.
    """
    n = d.get(f'time_{kind}_count')
    other = 'read' if kind == 'read' else 'other'
    nc = d.get(f'time_pump_commit_{other}_count')
    commit, taken = d.get(f'time_pump_commit_{other}_ns'), d.get(f'time_pump_taken_{other}_ns')
    stages = [
        ('fetched to submitted (pump, all kinds)', _per(d.get('time_pump_submit_ns'), d.get('time_pump_submit_count'))),
        ('submitted to begun (wait)', _per(d.get(f'time_{kind}_wait_ns'), n)),
        ('begun to ready (call)', _per(d.get(f'time_{kind}_call_ns'), n)),
        (f"ready to taken (pump, {'reads' if kind == 'read' else 'all but reads'})", _per(taken, nc)),
        (f"taken to committed (pump, {'reads' if kind == 'read' else 'all but reads'})",
         None if commit is None or taken is None else _per(commit - taken, nc)),
    ]
    return stages, n


def wait_by(d, kind):
    """-> ([(activity, ns)] largest first, their sum, the kind's wait)."""
    rows = [(a, d.get(f'time_{kind}_wait_by_{a}_ns')) for a in ACTIVITIES]
    rows = [(a, v) for a, v in rows if v is not None]
    rows.sort(key=lambda r: -r[1])
    return rows, sum(v for _, v in rows), d.get(f'time_{kind}_wait_ns')


# A wait's causes in groups: the hand-over to the worker; a checkpoint
# step and the rule's own sync; the log's sync for a flush; requests
# that write; other reads; the rest.
GROUPS = (('idle (hand-over to the worker)', ('idle',)),
          ('a step and the own sync', tuple('step_' + p for p in STEP_PARTS) + ('own_sync',)),
          ('flush', ('flush',)),
          ('write, discard, zeroes', ('write', 'discard', 'zeroes')),
          ('read', ('read',)),
          ('other', ('other',)))


def wait_groups(d, kind):
    """-> [(group, ns)] in the order of GROUPS; every activity is in exactly one group."""
    return [(name, sum(d.get(f'time_{kind}_wait_by_{a}_ns') or 0 for a in acts)) for name, acts in GROUPS]


def identities(t):
    """-> [(what, left, right, difference)]: the guard on the reading."""
    d, out = t['delta'], []
    for kind in KINDS:
        rows, total, wait = wait_by(d, kind)
        if rows and wait is not None:
            out.append((f"{kind}: the wait's causes sum to the wait (ns)", total, wait, total - wait))
    worker = [d.get(f'time_worker_{a}_ns') for a in ACTIVITIES]
    if all(v is not None for v in worker) and t.get('window_ns') is not None:
        out.append(("the worker's activities sum to the time between the two readings (ns)",
                    sum(worker), t['window_ns'], sum(worker) - t['window_ns']))
    return out


def render(rows):
    """rows: [{'label', 'workload', 'tps', 'time': delta(), 'steps', 'relation_read_us', 'relation_reads',
    'duration_s', 'ages': (age_ms before, after), 'model_relation_read_us'}] -> markdown lines."""
    out = []
    if not rows:
        return out
    out += ['', "Where a request's time goes in the daemon, over the measured window (differences of two readings of "
            '`skyzfsd status`, taken just before and just after it; observations, not judged). The stages of a '
            "request's life are fetched, submitted to the worker, begun, ready, taken by the pump, committed to the "
            'kernel:', '']
    usable = []
    for r in rows:
        t = r.get('time') or {}
        if not t.get('delta'):
            out.append(f"- {r['workload']}, {r['label']}: NO TIMES: {t.get('why') or 'the row has no reading'}")
        else:
            usable.append(r)
            if not t.get('ok'):
                out.append(f"- {r['workload']}, {r['label']}: THE READING IS DOUBTFUL: {t.get('why')}")
    if usable:
        out += ['', 'The window of each reading: the two readings are taken after a `sync` and 2 s on each side of '
                'pgbench, so they are further apart than pgbench ran, and the worker\'s idle time includes the rest:', '',
                '| workload | daemon row | the readings apart, s | pgbench ran, s |', '|---|---|---|---|']
        for r in usable:
            w = r['time'].get('window_ns')
            out.append(f"| {r['workload']} | {r['label']} | {_f(None if w is None else w / 1e9, '.1f')} | {_f(r.get('duration_s'), '.0f')} |")
    for kind in ('read', 'write', 'flush'):
        heads = None
        for r in usable:
            stages, n = life(r['time']['delta'], kind)
            if heads is None:
                heads = [s for s, _ in stages]
                extra = (' all reads in the daemon, s | postgres: relation reads | postgres: µs per relation read | '
                         'the model row\'s on this machine | postgres: s in relation reads | the daemon\'s share of it |'
                         if kind == 'read' else '')
                note = ('' if kind != 'read' else
                        ' The last four columns set the daemon\'s account beside postgres\'s own (pg_stat_io, object '
                        'relation: reads and read time, with track_io_timing; and the same mean of the model row, zfs-slog, where one '
                        'ran on this machine), as totals over the window and not per '
                        'read: a relation read is 8 KiB asked of ZFS, which the ARC may answer without the device, so '
                        'postgres counts more reads than the daemon and the two means are not of the same reads. What '
                        'of postgres\'s read time is not in the daemon\'s account is outside the daemon: the kernel, '
                        'the ring, ZFS, and the reads the ARC answered.')
                out += ['', f"A {kind}'s life, mean µs per {kind}.{note}", '',
                        f"| workload | daemon row | tps | {kind}s | " + ' | '.join(heads) + f' | sum |{extra}',
                        '|---|---|---|---|' + '---|' * (len(heads) + 1 + (6 if extra else 0))]
            vals = [v for _, v in stages]
            total = sum(v for v in vals if v is not None) if any(v is not None for v in vals) else None
            line = (f"| {r['workload']} | {r['label']} | {_f(r.get('tps'), '.0f')} | {_f(n, 'd') if n is not None else 'n/a'} | "
                    + ' | '.join(_f(v) for v in vals) + f' | {_f(total)} |')
            if kind == 'read':
                pg, pgn = r.get('relation_read_us'), r.get('relation_reads')
                ours = None if total is None or not n else total * n / 1e6
                theirs = None if pg is None or not pgn else pg * pgn / 1e6
                line += (f" {_f(ours)} | {_f(pgn, '.0f')} | {_f(pg)} | {_f(r.get('model_relation_read_us'))} | "
                         f"{_f(theirs)} | {_share(ours, theirs)} |")
            out.append(line)
        if not usable:
            continue
        out += ['', f"A {kind}'s wait (submitted to begun) by what the worker did meanwhile, µs per {kind} and share, "
                'largest first; `idle` is the worker waking up:', '', '| workload | daemon row | ' + f'{kind} wait, µs | by cause |',
                '|---|---|---|---|']
        for r in usable:
            d = r['time']['delta']
            causes, _, wait = wait_by(d, kind)
            n = d.get(f'time_{kind}_count')
            said = ', '.join(f'{a} {_f(_per(v, n))} ({_share(v, wait)})' for a, v in causes if v) or 'nothing waited'
            out.append(f"| {r['workload']} | {r['label']} | {_f(_per(wait, n))} | {said} |")
        out += ['', f"The same wait in groups, µs per {kind} and share:", '',
                '| workload | daemon row | ' + ' | '.join(g for g, _ in GROUPS) + ' |', '|---|---|' + '---|' * len(GROUPS)]
        for r in usable:
            d = r['time']['delta']
            n, wait = d.get(f'time_{kind}_count'), d.get(f'time_{kind}_wait_ns')
            out.append(f"| {r['workload']} | {r['label']} | "
                       + ' | '.join(f'{_f(_per(v, n))} ({_share(v, wait)})' for _, v in wait_groups(d, kind)) + ' |')
    if usable:
        out += ['', 'Flushes beside reads:', '', '| workload | daemon row | flushes | flushes per read | mean flush call, µs |',
                '|---|---|---|---|---|']
        for r in usable:
            d = r['time']['delta']
            nf_, nr = d.get('time_flush_count'), d.get('time_read_count')
            out.append(f"| {r['workload']} | {r['label']} | {_f(nf_, 'd') if nf_ is not None else 'n/a'} | "
                       f"{_f(None if not nr or nf_ is None else nf_ / nr, '.3f')} | {_f(_per(d.get('time_flush_call_ns'), nf_))} |")
        out += ['', "The worker's time by activity, as shares of all it counted in the window; `busy` is all but idle:", '',
                '| workload | daemon row | busy | ' + ' | '.join(ACTIVITIES) + ' |', '|---|---|---|' + '---|' * len(ACTIVITIES)]
        for r in usable:
            d = r['time']['delta']
            w = [d.get(f'time_worker_{a}_ns') for a in ACTIVITIES]
            whole = sum(v for v in w if v is not None)
            busy = None if w[0] is None else whole - w[0]
            out.append(f"| {r['workload']} | {r['label']} | {_share(busy, whole)} | " + ' | '.join(_share(v, whole) for v in w) + ' |')
        out += ['', 'Checkpoint steps in the window: ms per step of each part, the sync the step does itself, and the '
                'gate (turns of the worker in which a request waited and none was taken because the log was at its high '
                'mark; its time is a share of the parts, not more time), beside all the reads\' wait:', '',
                '| workload | daemon row | steps | steps per second | reads per step | ' + ' | '.join(STEP_PARTS) + ' | own sync | all parts | the walk, of the six parts | gate count | gate, ms in all | all reads\' wait, ms |',
                '|---|---|---|---|---|' + '---|' * (len(STEP_PARTS) + 6)]
        for r in usable:
            d, steps = r['time']['delta'], r.get('steps')
            parts = [d.get(f'time_step_{p}_ns') for p in STEP_PARTS] + [d.get('time_step_own_sync_ns')]
            whole = sum(v for v in parts if v is not None)
            w, nr = r['time'].get('window_ns'), d.get('time_read_count')
            out.append(f"| {r['workload']} | {r['label']} | {_f(steps, 'd') if steps is not None else 'n/a'} | "
                       f"{_f(None if not w or steps is None else steps / (w / 1e9), '.2f')} | "
                       f"{_f(None if not steps or nr is None else nr / steps, '.0f')} | "
                       + ' | '.join(_f(_per(v, steps, 1e6), '.2f') for v in parts)
                       + f" | {_f(_per(whole, steps, 1e6), '.2f')} | {_share(parts[0], sum(v for v in parts[:6] if v is not None))} | "
                       f"{_f(d.get('time_gate_count'), 'd') if d.get('time_gate_count') is not None else 'n/a'} | "
                       f"{_f(None if d.get('time_gate_ns') is None else d['time_gate_ns'] / 1e6, '.1f')} | "
                       f"{_f(None if d.get('time_read_wait_ns') is None else d['time_read_wait_ns'] / 1e6, '.1f')} |")
        out += ['', "The pump's store of what was acknowledged:", '',
                '| workload | daemon row | stores | ms each | completions behind one store |', '|---|---|---|---|---|']
        for r in usable:
            d = r['time']['delta']
            n = d.get('time_pump_store_count')
            out.append(f"| {r['workload']} | {r['label']} | {_f(n, 'd') if n is not None else 'n/a'} | "
                       f"{_f(_per(d.get('time_pump_store_ns'), n, 1e6), '.3f')} | "
                       f"{_f(None if not n or d.get('time_pump_store_completions') is None else d['time_pump_store_completions'] / n, '.2f')} |")
        out += ['', 'The longest of each since the daemon started serving (not of the window), ms, and what the longest '
                'wait of each kind was for:', '', '| workload | daemon row | ' + ' | '.join(f'{k} wait max' for k in KINDS[:3])
                + ' | ' + ' | '.join(f'{k} call max' for k in KINDS[:3]) + ' | '
                + ' | '.join(f'{k}: longest wait behind' for k in KINDS[:3]) + ' |', '|---|---|' + '---|' * 9]
        for r in usable:
            e = r['time']['end']
            ms = lambda v: _f(v / 1e6, '.2f') if isinstance(v, int) else 'n/a'
            cause = lambda k: (f"{e.get(f'time_{k}_longest_wait_cause', 'n/a')} "
                               f"({ms(e.get(f'time_{k}_longest_wait_cause_ns'))} ms)")
            out.append(f"| {r['workload']} | {r['label']} | " + ' | '.join(ms(e.get(f'time_{k}_wait_max_ns')) for k in KINDS[:3])
                       + ' | ' + ' | '.join(ms(e.get(f'time_{k}_call_max_ns')) for k in KINDS[:3])
                       + ' | ' + ' | '.join(cause(k) for k in KINDS[:3]) + ' |')
        out += ['', 'The identities, as a guard on the reading. A kind\'s causes must sum to its wait exactly: that is '
                'the guard that counts. The worker\'s activities are set against the time between the two looks at '
                'the clock, taken just before each reading. The sums are the worker\'s spans closed at its last '
                'publish; the span open at each reading (an idle wait or a step in progress) is not in them, so that '
                'identity holds to within one open span and the status file\'s age at each end, about 0.1% of the '
                'window (−3 to +141 ms in the first runs), and is not accounted for row by row:', '',
                '| workload | daemon row | identity | left | right | difference |', '|---|---|---|---|---|---|']
        for r in usable:
            ids = identities(r['time'])
            for what, left, right, diff in ids:
                ages = r.get('ages') or (None, None)
                if what.startswith("the worker's"):
                    what += f" (the readings' ages: {ages[0] if ages[0] is not None else 'not said'} ms and {ages[1] if ages[1] is not None else 'not said'} ms)"
                out.append(f"| {r['workload']} | {r['label']} | {what} | {left} | {right} | {diff} |")
            if not ids:
                out.append(f"| {r['workload']} | {r['label']} | none could be formed | | | |")
    return out
