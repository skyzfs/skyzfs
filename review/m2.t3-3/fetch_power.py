#!/usr/bin/env python3
"""#119 at 32d8d67 (M2.T3 stage 3): what the cuts node B fetched in the
kernel runs could tell apart.

    fetch_power.py CHECKOUT RUN_DIR [RUN_DIR ...]

CHECKOUT holds the head's tests/kernel (the judge is imported from it);
RUN_DIR is a run's artifact, unpacked (one directory per recording, with
requests.tsv, uploaded.tsv and the rest). Nothing in RUN_DIR is run.

For each cut of the daemon's that node B fetched (daemons_fetches, as
upload_lines takes them) it counts the blocks whose content, by the
request log's hashes, differs between the reference at the cut's instant
and:

- prev: the reference at the instant of the cut counted before it;
- next: at the instant of the cut counted after it (- for the last);
- live: every record the log has before node B's read began (the
  monotonic time in uploaded.tsv): what the volume held when node B
  looked, whatever the daemon uploaded;
- end: every record of the log.

A fetch with live 0 was made with the volume at rest since the cut's
instant: a daemon that read the volume as it was when it uploaded, and
not as it was at the cut's instant, shows the same image there. Only
blocks both models know as one hash are counted.
"""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests', 'kernel'))
import cut_daemon_check as cdc  # noqa: E402


def read(d, name):
    with open(os.path.join(d, name)) as f:
        return f.read()


def model_at(records, upto):
    m = {}
    for r in records[:upto]:
        cdc.apply(m, r)
    return m


def differ(a, b):
    n = 0
    for blk in set(a) | set(b):
        x, y = a.get(blk, cdc.ZERO), b.get(blk, cdc.ZERO)
        if isinstance(x, str) and isinstance(y, str) and x != y:
            n += 1
    return n


def index_at(records, ns):
    """One past the last record the log times at or before ns."""
    last = 0
    for r in records:
        t = r.get('ns')
        if t is not None and t <= ns:
            last = r['i'] + 1
    return last


def recording(rdir):
    name = os.path.basename(rdir)
    torn = []
    instances = cdc.parse_log(read(rdir, 'requests.tsv'), torn)
    kinds = cdc.read_kinds(read(rdir, 'instances.tsv'))
    acked = cdc.read_acknowledged(read(rdir, 'instances.tsv'))
    instances, _ = cdc.unstored_at_kill(instances, acked)
    records, dcuts = cdc.records_of(cdc.log_lines(instances, kinds, acked), cdc.read_marks(read(rdir, 'marks.tsv')))
    cdc.epochs(dcuts, records)
    got = cdc.read_uploaded(read(rdir, 'uploaded.tsv')) if os.path.exists(os.path.join(rdir, 'uploaded.tsv')) else []
    others = cdc.read_others(read(rdir, 'others.tsv')) if os.path.exists(os.path.join(rdir, 'others.tsv')) \
        else (set(), set())
    n_epochs = 1 + max((d.get('epoch', 0) for d in dcuts), default=0)
    offsets = [sum(1 for d in dcuts if d.get('epoch', 0) < e) for e in range(n_epochs)]
    mine, _, skipped, _, _ = cdc.daemons_fetches(got, read(rdir, 'uploaded-blocks.tsv') if got else '', others, offsets)
    by_number = {int(d['number']): k for k, d in enumerate(dcuts) if d['number'].isdigit()}
    status = cdc.read_status(read(rdir, 'status.txt'))
    moved = sum(1 for ln in read(rdir, 'watch.log').splitlines() if 'moved under the read' in ln)
    print(f"{name}: {len(dcuts)} cuts counted, uploaded {status.get('uploaded')} (the last instance), node B fetched "
          f"{len(mine)} of the daemon's and {skipped} of another writer's; {moved} reads given up because head moved")
    for g in mine:
        k = by_number.get(g['number'])
        if k is None:
            print(f"  cut {g['number']}: no cut line")
            continue
        d = dcuts[k]
        here = model_at(records, d['prefix'])
        prev = differ(here, model_at(records, dcuts[k - 1]['prefix'])) if k else '-'
        nxt = differ(here, model_at(records, dcuts[k + 1]['prefix'])) if k + 1 < len(dcuts) else '-'
        at = index_at(records, g['asked'])
        live = differ(here, model_at(records, max(at, d['prefix'])))
        end = differ(here, model_at(records, len(records)))
        writes = sum(1 for r in records[d['prefix']:max(at, d['prefix'])] if cdc.cut_ref.is_write(r))
        print(f"  cut {g['number']} (group {g['group']}), read {(g['asked'] - d['ns']) / 1e9:.1f} s after its cut line: "
              f"prev {prev}, next {nxt}, live {live} ({writes} writes applied between), end {end}")


def main():
    for run in sys.argv[2:]:
        print(f'== {os.path.basename(os.path.normpath(run))}')
        for name in sorted(os.listdir(run)):
            rdir = os.path.join(run, name)
            if os.path.isdir(rdir) and os.path.exists(os.path.join(rdir, 'requests.tsv')) \
                    and os.path.exists(os.path.join(rdir, 'watch.log')):
                try:
                    recording(rdir)
                except Exception as e:  # noqa: BLE001
                    print(f'{name}: {type(e).__name__}: {e}')


if __name__ == '__main__':
    main()
