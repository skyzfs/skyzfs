#!/usr/bin/env python3
"""#116 at 8e20e51 (the next look): more logs around a marking write, for
the rule `reapplied` has now (a flush the next instance received before
the write's second `applied` line had the stored mark, whether it cuts
before or after that line; a tag used again after that line is another
flush's). The prelude is windows.py's; W, F, G and K are as there.

    windows2.py CHECKOUT

  Mt  Ma, the other flush F answered, and G's own flush, received after
      W's second `applied` line, carrying F's tag (a tag is free again
      once its request is answered).                            RIGHT
  Mr  the mark stored; two flushes under way, both handed before W: the
      first received cuts K, the second nothing.                RIGHT
  Ms  as Mr, the cut at the second flush and not the first (the mark is
      the first flush's: it takes it at its receipt).           WRONG
  Mx  Ma, and G's own flush cuts nothing (the mark of the second
      application lost): K at the next group.                   WRONG
  My  Mb, and G's own flush cuts again, K + 1 at the same group. WRONG
  Mz  Mc, and G's own flush cuts again, K + 1 at the same group. WRONG
  Mv  the mark not stored; W handed first, F received while W is on the
      threads, and F cuts K at its barrier with the mark W's second
      application set (a daemon that takes the mark at the barrier and
      not at the receipt): the log is Mc's, line for line.      WRONG, not told from Mc
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


def retag(lines, i, tag):
    """The request of the cut line at index i of LINES, in lines, by another tag."""
    old = LINES[i].split('\t')[3]
    lo = i - 2
    out, seen = [], False
    for ln in lines:
        f = ln.split('\t')
        if ln == LINES[lo]:
            seen = True
        if seen and f[3] == old and f[4] == 'flush' and f[2] in ('recv', 'barrier', 'cut', 'commit'):
            f[3] = tag
            if f[2] == 'commit':
                seen = False
        out.append('\t'.join(f))
    assert out != list(lines)
    return out


FCOMMIT = f'0\t{PID}\tcommit\t{FTAG}\tflush\t0\t0\t0'
F2 = [ln.replace(f'\t{FTAG}\tflush', '\t8\tflush') for ln in (flush('recv'), flush('barrier'))]
KCUT2 = KCUT.replace(f'\tcut\t{FTAG}\t', '\tcut\t8\t')
assert KCUT2 != KCUT and F2[0] != flush('recv')
print(f'tags in use after W: F is {FTAG}, G\'s own flush is {LINES[CI].split(chr(9))[3]}')
judge('Mt', 'RIGHT', log(first_f, (PID2, [flush('recv'), flush('barrier'), FCOMMIT] + W + retag(AFTER, CI, FTAG))))
judge('Mr', 'RIGHT', log(first_f, (PID2, [flush('recv'), F2[0], flush('barrier'), KCUT, F2[1]] + W + NOCUT)))
judge('Ms', 'WRONG', log(first_f, (PID2, [flush('recv'), F2[0], flush('barrier'), F2[1], KCUT2] + W + NOCUT)))
judge('Mx', 'WRONG', log(first_f, (PID2, [flush('recv'), flush('barrier')] + W + renumber(NOCUT, 1))))
again_after = renumber(AFTER, -1, since=CI - (WI + 4))
judge('My', 'WRONG', log(first_f, (PID2, [flush('recv'), flush('barrier'), KCUT] + W + again_after)))
judge('Mz', 'WRONG', log(first_f, (PID2, W[:2] + [flush('recv'), W[2], flush('barrier'), KCUT, W[3]] + again_after)))
print('Mv (WRONG): the log is Mc\'s')
