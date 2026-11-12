"""The daemon's rows against the model rows around them.

A sequence runs its rows one after another on one machine. The machine
can change over a job: on runners with slow syncs, the model row at the
end of a sequence has measured from 0.985 to 1.270 times the first one,
as much as the effects the benchmark judges. So a daemon row is judged
against the two model rows that enclose it in its sequence, the nearest
before it and the nearest after it, and not against the first model row
of the sequence. Its bracketed ratio is

    the daemon row's tps / the mean of the two model rows' tps.

A daemon row with no model row before it or none after it, or that is
not in a sequence, has no bracketed ratio.

`enclose` finds the two rows; `bracketed` forms the ratio and says
whether it may be judged. Rows are the benchmark's result dicts, with
`_seq` (the row's place in its sequence, or None), `machine`,
`workload`, `topology`, `tps` and `origin_delay_ms`.
"""

MODEL = 'zfs-slog'


def same_place(a, b):
    """Whether two rows ran in one sequence on one machine, with one workload."""
    return (a.get('machine') is not None and a.get('machine') == b.get('machine')
            and a.get('workload') == b.get('workload'))


def enclose(row, rows, model=MODEL):
    """-> (the model row before `row`, the model row after it), either None.

    Among the rows of `row`'s machine and workload, in sequence order.
    Model rows without a tps (the run did not measure) and model rows
    with snapshots in their window are passed over.
    """
    seq = row.get('_seq')
    if seq is None:
        return None, None
    ms = [o for o in rows if o.get('topology') == model and o.get('_seq') is not None
          and o.get('tps') and o.get('snap', {}).get('mode', 'none') == 'none' and same_place(o, row)]
    before = max((o for o in ms if o['_seq'] < seq), key=lambda o: o['_seq'], default=None)
    after = min((o for o in ms if o['_seq'] > seq), key=lambda o: o['_seq'], default=None)
    return before, after


def bracketed(row, rows, model=MODEL, criterion=0.90):
    """-> dict: before, after (rows or None), ratio (float or None), verdict (text).

    The verdict is 'at least 0.90' or 'BELOW 0.90' only when the row is
    enclosed and both model rows had no origin delay (the daemon row has
    no origin under it, and a model row slowed at its origin reads would
    flatter the daemon). Otherwise it starts with NO VERDICT and says why.
    """
    out = {'before': None, 'after': None, 'ratio': None}
    if not row.get('tps'):
        out['verdict'] = 'no tps: the run did not measure'
        return out
    if not row.get('machine'):
        out['verdict'] = 'NO VERDICT: this row does not say what machine it ran on'
        return out
    if row.get('_seq') is None:
        out['verdict'] = 'NO VERDICT: the row is not in a sequence, so no model rows enclose it'
        return out
    before, after = enclose(row, rows, model)
    out['before'], out['after'] = before, after
    if before is None or after is None:
        missing = ' and '.join(w for w, o in (('before', before), ('after', after)) if o is None)
        out['verdict'] = f'NO VERDICT: no {model} row of this machine and workload {missing} it in the sequence'
        return out
    out['ratio'] = row['tps'] / ((before['tps'] + after['tps']) / 2)
    delays = [before.get('origin_delay_ms'), after.get('origin_delay_ms')]
    if any(d is None for d in delays):
        out['verdict'] = 'NO VERDICT: a model row does not say what origin delay it had'
    elif any(d != 0 for d in delays):
        out['verdict'] = (f'NO VERDICT: the model rows had origin delays of {delays[0]} and {delays[1]} ms, '
                          'the daemon row has no origin')
    else:
        out['verdict'] = f'at least {criterion:.2f}' if out['ratio'] >= criterion else f'BELOW {criterion:.2f}'
    return out
