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

A row that says it uploads (TB_SKZ_UPLOAD) keeps its ratio, printed,
and has no verdict when its window was not a window of a daemon that
uploads (review's G4 on #123, the lead's decision (c)): no upload
committed in it, the volume's lease was not held at both of its ends
under one epoch, or the daemon was not the same at its two ends.

A row whose seed was let go at its first window (TB_SKZ_UPLOAD_FROM_LOAD
=window) measures the seed's cost in that window, so a lease taken in
it counts (the lead's decision on #132): such a row is judged when its
own window let MinIO go (`released_here`; a later row whose window
saw the tail of a seed released earlier is not such a row: Codex on
#132), the volume had no lease at the window's start (`none`: MinIO
was frozen until after that read), no upload had committed by then, a
whole upload had by its end, and the lease was held at its end. Its
verdict says "seed inside window, lease at release" and the lease's
epoch. Any other row keeps the rule above.

A seed row is a row that says it uploads and had committed no upload
at its window's start (`uploaded` 0 there), or no upload after its
seed (`uploaded` at most `uploaded_whole` there): whatever it uploaded
in the window began with the seed, or with the first upload after it,
which carries every block written while the seed ran (review's D6 on
#132). Its verdict stands on its own line
and is not one of the median of design §9, which is the steady
state's ratio (the lead's decision on #132); `bracketed` says so in
`seed_row`.

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


# The lease as the status says it while the uploader holds it: "unsure"
# is held, its last renewal failed, and the uploads go on under its epoch.
LEASE_HELD = ('held', 'unsure')


def one_server(k):
    """-> whether a daemon row's window had one server, as its tables say it.

    Read as not_uploading reads it: when either end's status shows no
    pid, 'no serving daemon at an end' if such an end was read (it has
    fields beside the clock line `read_at_ns`: a daemon that died in
    the window), else 'status not read' (a reading that failed leaves no
    more than its clock line; review of #132, D2). An end that says
    `state serving` with no pid is 'status not read' too: the device is
    live, and the status command prints its state and no more when the
    daemon's status file cannot be read (review of #132, D8). 'NO: the
    pid changed' only when both pids were read and differ, else 'yes'
    (review of #128, L1).
    """
    sb, sa = k.get('before') or {}, k.get('after') or {}
    nopid = [s for s in (sb, sa) if not s.get('pid')]
    if nopid:
        read = [s for s in nopid if set(s) - {'read_at_ns', 'pid'} and s.get('state') != 'serving']
        return 'no serving daemon at an end' if read else 'status not read'
    return 'yes' if sb['pid'] == sa['pid'] else 'NO: the pid changed'


def not_uploading(row):
    """-> why a row that says it uploads did not upload over its window, or None.

    None also for a row that does not say it uploads. Read from the
    daemon's status at the window's two ends (`skyzfsd.before` and
    `.after`) and whether its pid was the same at both (`pid_same`). A
    status not read at either end (no pid in it) shows no upload, and an
    epoch that is not a number shows no one lease: no verdict.
    """
    k = row.get('skyzfsd') or {}
    if not k.get('upload'):
        return None
    sb, sa = k.get('before') or {}, k.get('after') or {}
    # A reading that failed leaves its clock line (`read_at_ns`) and no
    # more: a status read shows the daemon's pid.
    unread = [end for end, s in (('start', sb), ('end', sa)) if not s.get('pid')]
    if unread:
        return f"its daemon's status was not read, or showed no serving daemon, at the window's {' and '.join(unread)}"
    if k.get('pid_same') is not True:
        return (f"its daemon was not shown to be the same at the window's two ends (pid {sb['pid']} "
                f"and {sa['pid']})")
    lease = [(s.get('lease', 'not read'), s.get('lease_epoch', 'not read')) for s in (sb, sa)]
    if seed_in_window(row):
        return None
    if (any(state not in LEASE_HELD for state, _ in lease) or lease[0][1] != lease[1][1]
            or not str(lease[0][1]).isdigit()):
        seed = ''
        if (k.get('upload') or {}).get('from_load') == 'window' and k.get('released_here') is not True:
            # A row recorded before the mark has none, though its window
            # may have let MinIO go (review of #132, D7).
            seed = ("; its window is not marked as the one that let MinIO go, so its seed was not measured from its "
                    "release")
        elif (k.get('upload') or {}).get('from_load') == 'window':
            seed = (f"; its seed, let go at the window's start, was not shown to commit inside it under the lease held "
                    f"at its end (uploaded {sb.get('uploaded', 'not read')} at its start, "
                    f"{sa.get('uploaded_whole', 'not read')} whole at its end)")
        return (f'it did not hold the lease for the whole window (at its start {lease[0][0]} at epoch {lease[0][1]}, '
                f'at its end {lease[1][0]} at epoch {lease[1][1]}){seed}')
    a, b = sa.get('uploaded', ''), sb.get('uploaded', '')
    if not (a.isdigit() and b.isdigit()):
        return f"its uploads cannot be read at the window's two ends ('{b}' and '{a}')"
    if int(a) - int(b) <= 0:
        return f'no upload committed in its window (uploaded {b} at its start and {a} at its end)'
    return None


def seed_row(row):
    """-> why a row is a seed row (not in design §9's median), or None.

    A row that says it uploads whose status shows `uploaded` 0 at its
    window's start: no upload had committed, so what it uploaded in the
    window began with the seed (the lead's decision on #132). Or one
    whose status shows no more uploads than whole ones there: the seed
    had committed (as whole uploads) and nothing after it, so the
    window's first upload carries every block written while the seed
    ran (review's D6 on #132, the lead's decision)."""
    k = row.get('skyzfsd') or {}
    if not k.get('upload'):
        return None
    sb = k.get('before') or {}
    u, w = str(sb.get('uploaded', '')), str(sb.get('uploaded_whole', ''))
    if u == '0':
        return 'no upload had committed at its window\'s start'
    if u.isdigit() and w.isdigit() and int(u) <= int(w):
        return (f"no upload after the seed had committed at its window's start (uploaded {u}, {w} of them whole), "
                'so its first upload carries what was written while the seed ran')
    return None


def seed_in_window(row):
    """-> 'seed inside window, lease at release (epoch N)' for a row whose
    seed was let go at its window's start and committed inside it under
    the lease it took in the window; else None.

    Only the row whose window let MinIO go (`released_here`, from the
    file topology-bench.sh's skz_minio_resume writes for that window): a
    seed that outlived its window and committed in a later one is not
    measured from its release there (Codex on #132).

    The release comes after the status read at the window's start
    (topology-bench.sh, skz_minio_resume), so no upload at that read and
    a whole one at the end's is a seed committed inside the window. The
    lease must be `none` at the start: in the window that let MinIO go
    the daemon cannot have taken it before that read, so a lease held
    there is not one taken at the release (review of #132, D1, the
    lead's option (i)). Read only when the daemon was read at both ends
    and was the same (not_uploading).
    """
    k = row.get('skyzfsd') or {}
    if (k.get('upload') or {}).get('from_load') != 'window' or k.get('released_here') is not True:
        return None
    sb, sa = k.get('before') or {}, k.get('after') or {}
    if not (sb.get('pid') and sa.get('pid') and k.get('pid_same') is True):
        return None
    end = (sa.get('lease', 'not read'), sa.get('lease_epoch'))
    if end[0] not in LEASE_HELD or not str(end[1]).isdigit():
        return None
    # The start's epoch is not read: with no lease there, it has none.
    if sb.get('lease', 'not read') != 'none':
        return None
    if sb.get('uploaded') != '0' or not str(sa.get('uploaded_whole', '')).isdigit() or int(sa['uploaded_whole']) < 1:
        return None
    return f'seed inside window, lease at release (epoch {end[1]})'


def bracketed(row, rows, model=MODEL, criterion=0.90, widest=WIDEST):
    """-> dict: before, after (rows or None), ratio and width (floats or None), verdict (text).

    The verdict is 'at least 0.90' or 'BELOW 0.90' only when the row is
    enclosed, its two model rows ran with its settings (`SETTINGS`),
    differ by at most `widest` of their mean, and both had no origin
    delay (the daemon row has
    no origin under it, and a model row slowed at its origin reads would
    flatter the daemon), and a row that says it uploads uploaded over its
    window (`not_uploading`). Otherwise it starts with NO VERDICT and says
    why; a row that did not upload keeps its ratio and width. `seed_row`
    says why the row is a seed row, whose verdict is not one of design
    §9's median, or is None.
    """
    out = {'before': None, 'after': None, 'ratio': None, 'width': None, 'seed_row': seed_row(row)}
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
    ended = (row.get('skyzfsd') or {}).get('release', {}).get('ended')
    if ended:
        out['verdict'] = f'NO VERDICT: its releaser of held cuts (TB_SKZ_RELEASE) ended before the window did ({ended})'
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
    idle = not_uploading(row)
    idle = idle and f'NO VERDICT: the row says it uploads, but {idle}'
    before, after = enclose(row, rows, model)
    out['before'], out['after'] = before, after
    if before is None or after is None:
        missing = ' and '.join(w for w, o in (('before', before), ('after', after)) if o is None)
        out['verdict'] = idle or f'NO VERDICT: no {model} row of this machine and workload {missing} it in the sequence'
        return out
    out['ratio'] = row['tps'] / ((before['tps'] + after['tps']) / 2)
    out['width'] = width(before, after)
    if idle:
        out['verdict'] = idle
        return out
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
        seed = seed_in_window(row)
        if seed:
            out['verdict'] += f' ({seed})'
    return out
