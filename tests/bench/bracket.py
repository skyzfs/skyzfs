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

A bracket is too wide when its two model rows differ by more than a
tenth of their mean: the machine moved by more than the effects judged
while the row ran between them. Such a row keeps its ratio, printed,
and has no verdict ("bracket too wide"), so it gives nothing to a
median of verdicts.

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


WIDEST = 0.10

# The settings a model row must share with the daemon row it brackets:
# pgbench's clients and measured seconds, and postgres's memory limit,
# which bounds postgres in a model row as in a daemon row. A key that
# a row does not carry (rows recorded before it was) is not compared.
SETTINGS = (('clients', 'clients'), ('duration_s', 'measured seconds'), ('mem_limit', 'TB_MEM_LIMIT'))


def unlike(row, other):
    """-> the settings in which `other` differs from `row`, as text; empty if none."""
    return [f'{name} {other[k]} against {row[k]}' for k, name in SETTINGS
            if k in row and k in other and row[k] != other[k]]


def width(before, after):
    """How far the two model rows differ, as a share of their mean."""
    return abs(before['tps'] - after['tps']) / ((before['tps'] + after['tps']) / 2)


def bracketed(row, rows, model=MODEL, criterion=0.90, widest=WIDEST):
    """-> dict: before, after (rows or None), ratio and width (floats or None), verdict (text).

    The verdict is 'at least 0.90' or 'BELOW 0.90' only when the row is
    enclosed, its two model rows ran with its settings (`SETTINGS`),
    differ by at most `widest` of their mean, and both had no origin
    delay (the daemon row has
    no origin under it, and a model row slowed at its origin reads would
    flatter the daemon). Otherwise it starts with NO VERDICT and says why.
    """
    out = {'before': None, 'after': None, 'ratio': None, 'width': None}
    if not row.get('tps'):
        out['verdict'] = 'no tps: the run did not measure'
        return out
    if not row.get('machine'):
        out['verdict'] = 'NO VERDICT: this row does not say what machine it ran on'
        return out
    if row.get('slowed_by') and not row.get('syscalls') and not row.get('perf_record'):
        out['verdict'] = (f"NO VERDICT: its daemon was traced or recorded ({', '.join(row['slowed_by'])}), "
                          'which slows it, though the output is missing')
        return out
    if row.get('syscalls'):
        out['verdict'] = 'NO VERDICT: its system calls were traced (TB_SYSCALLS), which slows the daemon'
        return out
    if row.get('perf_record'):
        out['verdict'] = 'NO VERDICT: the daemon was recorded by perf (TB_PERF_RECORD), which slows it'
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
    out['width'] = width(before, after)
    other = [f'the model row {w} it: ' + ', '.join(d) for w, o in (('before', before), ('after', after))
             for d in [unlike(row, o)] if d]
    delays = [before.get('origin_delay_ms'), after.get('origin_delay_ms')]
    if other:
        out['verdict'] = 'NO VERDICT: a model row ran with other settings (' + '; '.join(other) + ')'
    elif any(d is None for d in delays):
        out['verdict'] = 'NO VERDICT: a model row does not say what origin delay it had'
    elif any(d != 0 for d in delays):
        out['verdict'] = (f'NO VERDICT: the model rows had origin delays of {delays[0]} and {delays[1]} ms, '
                          'the daemon row has no origin')
    elif out['width'] > widest:
        out['verdict'] = (f'NO VERDICT: bracket too wide: the model rows differ by {out["width"]:.1%} of their mean '
                          f'(at most {widest:.0%})')
    else:
        out['verdict'] = f'at least {criterion:.2f}' if out['ratio'] >= criterion else f'BELOW {criterion:.2f}'
    return out
