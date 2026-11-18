#!/usr/bin/env python3
"""#116 at 49c5f62: what the check says of logs a daemon killed around a
marking write (window M, the answer to F2) or around a cut's flush (window
C after R1) could leave. Built on the fixture `life`'s log, split in two
or three instances, judged with the check's own functions as the unit
tests do (unstored_at_kill, log_lines, records_of, the reference,
compare, numbers_verdict).

    windows.py CHECKOUT

Each log says whether the daemon that left it is right or wrong by my
reading of the store (crates/skyzfs-local/src/inplace.rs: `write` and the
threads' completions store the mark after the `applied` line and before
the answer; `accept` and `must_wait`: a flush waits for every write on
the threads, and what is received behind a waiting flush waits behind
it; inplace/cuts.rs: a flush takes the mark at its receipt and counts the
cut at its barrier; `take_up_cut_state`: a created device drops the
mark and keeps the held cuts).

W is the first uberblock write of the group G of cut K; F is another
flush under way at the kill (a ZIL flush: ZFS sends its own flush after
the uberblock writes only once they are answered).

Window M: the first instance ends on W's `applied` line.
  Ma  the mark not stored; the next is handed F first, cuts nothing
      there, applies W again and cuts K at G's own flush.       RIGHT
  Mb  the mark stored; handed F first, cuts K there.            RIGHT
  Mc  the mark stored; handed W first and F while W is on the threads:
      F took the state's mark at its receipt, waits for W, and cuts K at
      its barrier, after W's second `applied` line.             RIGHT
  Md  as Mc with the mark not stored: F carried nothing, no cut at its
      barrier; K at G's own flush.                              RIGHT
  Me  two groups with no flush between (as the course's noflush spans
      leave): the state holds the mark of the group before G, and G's
      was not stored. Handed F first, the next cuts the older group
      there, then applies W again and cuts G at its own flush.  RIGHT
  Mf  as Me with no F under way: W applied again, G cut at its flush
      (the older mark replaced).                                RIGHT
  Mg  as Ma, the next instance killed before it applied W again; a
      third is handed F first, cuts nothing there, applies W.   RIGHT
  Mh  as Ma, the next instance having created the device: nothing is
      handed to it, the state had no mark, G's later writes of the same
      uberblock mark, and G's own flush cuts.                   RIGHT
  Mw  the mark stored and dropped by the next instance at its take-over
      (the defect course (b) of `takeover` is there for): the log is
      Ma's, line for line.                                      WRONG, not told from Ma

Window C after R1: the first instance ends on F_G's barrier and `cut K`
(F_G is G's own flush).
  Ca  not stored; taken over, handed F_G again, cuts K there.   RIGHT
  Cb  not stored; taken over, handed F_G again and cuts nothing there
      (the mark dropped at the take-over); K at the next group. WRONG
  Cc  not stored; the next created the device: the mark is dropped,
      nothing is handed again, K at the next group.             RIGHT
  Cd  stored; taken over, handed F_G again, no cut; then K + 1. RIGHT
  Ce  stored, and the next instance counts K again at the next group
      on a device it took over (a cut lost at the start).       WRONG
  Cf  as Ce on a created device: line for line Cc.              WRONG, not told from Cc
  Cg  not stored; the next (taken over) killed after F_G's receipt;
      a third created the device and cuts K at the next group.  RIGHT
  Ch  as Cg, the third having taken the device over and cutting
      nothing at F_G: K at the next group.                      WRONG
"""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as cdc  # noqa: E402
import cut_ref  # noqa: E402
import test_cut_daemon_check as t  # noqa: E402

PID, PID2, PID3 = t.PID, t.PID2, '4545'
records, read, dev = t.fixture('life')
cs, at = t.daemon_cuts_of(records, read, dev)
TEXT, MARKS = t.request_log(records, read, dev, at, hashes=True)
LINES = TEXT.splitlines()
K = len(cs) // 2
CUTS = [i for i, ln in enumerate(LINES) if ln.split('\t')[2] == 'cut']
CI = CUTS[K - 1]
WI = next(i for i in range(CUTS[K - 2], CI) if LINES[i].split('\t')[2] == 'label') - 1
assert [ln.split('\t')[2] for ln in LINES[WI:WI + 4]] == ['recv', 'label', 'applied', 'commit']
assert [LINES[CI - 2].split('\t')[2], LINES[CI - 1].split('\t')[2]] == ['recv', 'barrier']
FTAG = '9'


def ev(i):
    return LINES[i].split('\t')[2]


def flush(what, cut=None):
    """A line of the other flush F."""
    if what == 'recv':
        return f'0\t{PID}\trecv\t{FTAG}\tflush\t0\t0\t-'
    if what == 'barrier':
        return f'0\t{PID}\tbarrier\t{FTAG}\tflush\t-\t-\t-'
    f = LINES[cut].split('\t')
    return '\t'.join(['0', PID, 'cut', FTAG] + f[4:])


def renumber(lines, minus, since=0):
    """The cut lines' numbers less `minus`, from line index `since` on."""
    out = []
    for j, ln in enumerate(lines):
        f = ln.split('\t')
        if f[2] == 'cut' and j >= since:
            f[6] = str(int(f[6]) - minus)
        out.append('\t'.join(f))
    return out


def instance(ns, pid, lines, how='taken_up'):
    out = [f'{ns + 1}\t{pid}\tstart\t0\t-\t-\t-\t64', f'{ns + 2}\t{pid}\tcuts\t-\t-\t-\t-\t{how}']
    at = ns + 2
    for ln in lines:
        f = ln.split('\t')
        f[1] = pid
        at += 1
        f[0] = str(at)
        out.append('\t'.join(f))
    return out, at


def log(first, *later):
    """first: the first instance's lines as they stand; later: (pid, lines)."""
    out = list(first)
    ns = int(out[-1].split('\t')[0])
    # the first instance's own added lines get times too
    fixed, at = [], 0
    for ln in out:
        f = ln.split('\t')
        at = int(f[0]) if int(f[0]) > at else at + 1
        f[0] = str(at)
        fixed.append('\t'.join(f))
    out, ns = fixed, at
    for pid, lines in later:
        more, ns = instance(ns, pid, lines)
        out += more
    return '\n'.join(out) + '\n'


def judge(name, verdict, text, kinds=None, show=True):
    kinds = kinds or {PID2: True}
    inst, notes = cdc.unstored_at_kill(cdc.parse_log(text))
    recs, dcuts = cdc.records_of(cdc.log_lines(inst, kinds), cdc.read_marks(MARKS))
    rread = cdc.read_of(recs, dev)
    rcs, _ = cut_ref.cuts(recs, cut_ref.scan(recs, rread, dev))
    ref = cdc.reference(rcs, cut_ref.opens(recs, rread, dev, rcs))
    ok, detail = cdc.compare(dcuts, ref)
    nv = cdc.numbers_verdict(dcuts)
    missing = cdc.cut_line_missing(dcuts, ref, recs)
    passed = bool(ok) and nv[0] == 'PASS' and missing is None
    print(f'{name} ({verdict}): {"passed" if passed else "FAILED"}; shapes {[n["shape"] for n in notes]}; '
          f'cuts {"as the reference" if ok else "DIFFER: " + detail[:230]}; cut_numbers {nv[0]}'
          + (f': {nv[1][:120]}' if nv[0] != 'PASS' else '')
          + (f'; cut_line_missing: {missing[:80]}' if missing else ''))
    return passed


# ---- window M ----
W = LINES[WI:WI + 4]            # recv, label, applied, commit
HEAD = LINES[:WI + 2]           # ... recv W, label
AFTER = LINES[WI + 4:]          # after W's commit: the group's other uberblock writes, its flush with cut K, ...
NOCUT = [ln for j, ln in enumerate(AFTER, WI + 4) if j != CI]    # the same, G's own flush cutting nothing
KCUT = flush('cut', CI)

first_plain = HEAD + [W[2]]
first_f = HEAD + [flush('recv'), W[2]]      # F received while W was on the threads

judge('Ma', 'RIGHT', log(first_f, (PID2, [flush('recv'), flush('barrier')] + W + AFTER)))
judge('Mb', 'RIGHT', log(first_f, (PID2, [flush('recv'), flush('barrier'), KCUT] + W + NOCUT)))
judge('Mc', 'RIGHT', log(first_f, (PID2, W[:2] + [flush('recv'), W[2], flush('barrier'), KCUT, W[3]] + NOCUT)))
judge('Md', 'RIGHT', log(first_f, (PID2, W[:2] + [flush('recv'), W[2], flush('barrier'), W[3]] + AFTER)))

# Me, Mf: the group before G synced with no flush: its own flush (cut K - 1)
# and every flush between it and W taken out of the first instance.
PI = CUTS[K - 2]
assert [ev(PI - 2), ev(PI - 1)] == ['recv', 'barrier']
gone = {PI - 2, PI - 1, PI}
for j in range(PI + 1, WI):
    f = LINES[j].split('\t')
    if f[4] == 'flush':
        gone.add(j)
older = [ln for j, ln in enumerate(LINES[:WI + 2]) if j not in gone]
PCUT = flush('cut', PI)
judge('Me', 'RIGHT', log(older + [flush('recv'), W[2]],
                         (PID2, [flush('recv'), flush('barrier'), PCUT] + W + AFTER)))
judge('Mf', 'RIGHT', log(older + [W[2]], (PID2, W + renumber(AFTER, 1))))

judge('Mg', 'RIGHT', log(first_f, (PID2, W[:2] + [flush('recv')]),
                         (PID3, [flush('recv'), flush('barrier')] + W + AFTER)),
      kinds={PID2: True, PID3: True})
judge('Mh', 'RIGHT', log(first_plain, (PID2, AFTER)), kinds={PID2: False})
print('Mw (WRONG): the log is Ma\'s')

# ---- window C ----
FG = LINES[CI - 2:CI + 1]       # recv, barrier, cut K
C_FIRST = LINES[:CI + 1]
REST = LINES[CI + 1:]
judge('Ca', 'RIGHT', log(C_FIRST, (PID2, FG + REST)))
judge('Cb', 'WRONG', log(C_FIRST, (PID2, FG[:2] + renumber(REST, 1))))
judge('Cc', 'RIGHT', log(C_FIRST, (PID2, renumber(REST, 1))), kinds={PID2: False})
judge('Cd', 'RIGHT', log(C_FIRST, (PID2, FG[:2] + REST)))
judge('Ce', 'WRONG', log(C_FIRST, (PID2, FG[:2] + renumber(REST, 1))))
print('Cf (WRONG): the log is Cc\'s')
judge('Cg', 'RIGHT', log(C_FIRST, (PID2, FG[:1]), (PID3, renumber(REST, 1))), kinds={PID2: True, PID3: False})
judge('Ch', 'WRONG', log(C_FIRST, (PID2, FG[:1]), (PID3, FG[:2] + renumber(REST, 1))),
      kinds={PID2: True, PID3: True})
