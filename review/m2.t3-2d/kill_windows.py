#!/usr/bin/env python3
"""#115 at afd217a: what the check says of a daemon killed around a flush,
on the fixture `life`'s log split in two instances at its 13th cut, run
through the program as the recording `takeover` (the unit tests' own
helper, test_cut_daemon_main.recording).

    kill_windows.py CHECKOUT

The pull request's four shapes (test_cut_daemon_check.split_log):
  A       the barrier line last, the next instance counts on from 14
  B       the barrier line last, the next instance cuts 13 again
  C       the barrier and `cut 13` last, the next instance cuts 13 again
  stored  the barrier and `cut 13` last, the next instance's next cut is 14

And logs of my own, each of a right daemon by my reading of the store
(inplace.rs `flush` and `write`, cuts.rs, state.rs `freeze` and `settle`):
  N   killed while a flush that carried no cut was syncing: its barrier
      line is the first instance's last (the flush before the uberblock
      writes of group 26). No cut was ever at stake.
  M0  killed after the `applied` line of the first uberblock write of
      group 26 and before the state stored the mark (`cuts.applied`
      writes the line, `record_cuts` stores the mark after it). The
      kernel hands the write to the next instance again, which applies
      it and marks; the flush after it cuts 13.
  M1  as M0, and the next instance receives another flush first (one in
      flight at the kill, as a ZIL flush would be), which carries
      nothing in the daemon: the state had no mark when it was received.
  C2  as C, but the next instance is killed too before the flush reaches
      its barrier, and a third instance cuts 13.
  Cc  as C, but the next instance created the device (no take-over): the
      mark is dropped, nothing is handed to it again, and its first cut
      is 13 again at a later group. By the check's functions, with no
      holder.

For each: the exit, obs_unstored_at_kill, and the lines that are not
PASS, INFO, CAUGHT or SAME or that say DIFFERS. held_across_start is left
out: it is ERROR in every one, the fixture's holder writing each cut
beside the instance that counted it. ctl_mark-kept DIFFERS in every one
too, as in the pull request's own tests (the fixture has no created
device with a mark waiting): an exit 1 with that line alone is a right
daemon passed."""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import test_cut_daemon_check as t  # noqa: E402
import test_cut_daemon_main as m  # noqa: E402

K = 13
PID3 = '4545'


def parts(text):
    lines = text.rstrip('\n').split('\n')
    j = next(i for i, ln in enumerate(lines) if ln.split('\t')[1:3] == [t.PID2, 'start'])
    return lines[:j], lines[j:]


def second(ns, lines, pid=None):
    """lines as the next instance's, each after the one before it."""
    pid = pid or t.PID2
    out = [f'{ns + 1}\t{pid}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{pid}\tcuts\t-\t-\t-\t-\ttaken_up']
    at = ns + 2
    for ln in lines:
        f = ln.split('\t')
        f[1] = pid
        at = int(f[0]) if int(f[0]) > at else at + 1
        f[0] = str(at)
        out.append('\t'.join(f))
    return out


def whole(text):
    """The split log (shape None: killed after the flush's recv) as one
    instance's lines again."""
    first, rest = parts(text)
    return first + [ln.replace(f'\t{t.PID2}\t', f'\t{t.PID}\t', 1) for ln in rest[3:]]


def shape_n(text):
    # one instance again, then cut at the barrier before the uberblock writes
    lines = whole(text)
    ci = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut'][K - 1]
    b = max(i for i in range(ci - 1) if lines[i].split('\t')[2] == 'barrier')
    assert lines[b - 1].split('\t')[2:5] == ['recv', lines[b].split('\t')[3], 'flush']
    ns = int(lines[b].split('\t')[0])
    return '\n'.join(lines[:b + 1] + second(ns, lines[b - 1:b + 1] + lines[b + 1:])) + '\n'


def shape_m(extra):
    def f(text):
        lines = whole(text)
        ci = [i for i, ln in enumerate(lines) if ln.split('\t')[2] == 'cut'][K - 1]
        b = max(i for i in range(ci - 1) if lines[i].split('\t')[2] == 'barrier')
        # recv, label, applied of the first uberblock write after it
        assert [lines[b + i].split('\t')[2] for i in (1, 2, 3, 4)] == ['recv', 'label', 'applied', 'commit']
        ns = int(lines[b + 3].split('\t')[0])
        again = lines[b + 1:b + 4]
        more = []
        if extra:
            more = [f'0\t{t.PID2}\trecv\t7\tflush\t0\t0\t-', f'0\t{t.PID2}\tbarrier\t7\tflush\t-\t-\t-',
                    f'0\t{t.PID2}\tcommit\t7\tflush\t0\t0\t0']
        return '\n'.join(lines[:b + 4] + second(ns, more + again + lines[b + 4:])) + '\n'
    return f


def shape_c2(text):
    first, rest = parts(text)
    # rest: start, cuts, recv, barrier, cut 13, ...; the second instance ends after the recv
    ns = int(rest[2].split('\t')[0])
    return '\n'.join(first + rest[:3] + second(ns, rest[2:], PID3)) + '\n'


def created():
    """Cc, by the check's functions (no holder: the numbers after 13 move)."""
    import cut_daemon_check as cdc
    import cut_ref
    records, read, dev = t.fixture('life')
    cs, at = t.daemon_cuts_of(records, read, dev)
    text, marks = t.request_log(records, read, dev, at, hashes=True)
    first, rest = parts(t.split_log(text, K, shape='C'))
    assert [ln.split('\t')[2] for ln in rest[2:5]] == ['recv', 'barrier', 'cut']
    out = []
    for ln in rest[:2] + rest[5:]:
        f = ln.split('\t')
        if f[2] == 'cut':
            f[6] = str(int(f[6]) - 1)
        out.append('\t'.join(f))
    inst, notes = cdc.unstored_at_kill(cdc.parse_log('\n'.join(first + out) + '\n'))
    recs, dcuts = cdc.records_of(cdc.log_lines(inst, {t.PID2: False}), cdc.read_marks(marks))
    rread = cdc.read_of(recs, dev)
    rcs, _ = cut_ref.cuts(recs, cut_ref.scan(recs, rread, dev))
    ref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
    print('Cc (C, the next instance created the device; its first cut is 13 again, at a later group):')
    print(f'   unstored_at_kill: {len(notes)} notes')
    print(f'   cuts: {cdc.compare(dcuts, ref)}')
    print(f'   cut_numbers: {cdc.numbers_verdict(dcuts)}')


def run(name, split, tamper=None, kind='taken-over', third=None, counted=K - 1):
    """counted: the cuts the state held at the second start, for its status."""
    none = lambda written, blocks, final: None    # noqa: E731
    records, read, dev = t.fixture('life')
    cs, _ = t.daemon_cuts_of(records, read, dev)
    status = m.held_status(records, read, dev, cs).replace('cut_state begun', 'cut_state taken_up')
    saved = m.t.holder_files

    def files(d, written, blocks, final, starts=(), **kw):
        starts = (starts[0], starts[1].replace(f'cuts {K - 1}\n', f'cuts {counted}\n'))
        if third:
            with open(os.path.join(d, 'instances.tsv'), 'a') as f:
                f.write(f'{PID3}\ttaken-over\n')
            starts += (starts[-1],)
        return saved(d, written, blocks, final, starts=starts, **kw)
    m.t.holder_files = files
    try:
        rc, lines = m.recording(holder=none, split=(K, kind, split), status=status, as_name='takeover',
                                tamper_split=tamper)
    finally:
        m.t.holder_files = saved
    print(f'{name}: exit {rc}')
    for key, (verdict, detail) in lines.items():
        if key == 'held_across_start':
            continue    # the fixture's holder writes each cut beside the instance that counted it
        if key == 'obs_unstored_at_kill' or verdict not in ('PASS', 'INFO', 'CAUGHT', 'SAME') or 'DIFFERS' in detail:
            print(f'   {key}: {verdict}: {detail[:330]}')


run('A', 'A')
run('B', 'B')
run('C', 'C')
run('stored', 'stored', counted=K)
run('N (a flush with no cut, killed while it synced)', None, shape_n)
run('M0 (killed between the applied line and the mark\'s store)', None, shape_m(False))
run('M1 (as M0, another flush received first by the next instance)', None, shape_m(True))
run('C2 (C, the next instance killed before the barrier, a third cuts)', 'C', shape_c2, third=True)
created()
