#!/usr/bin/env python3
"""Check c's third pass, volume by volume: did a checkpoint step fall
between the calls of each case?

    python3 -I tests/kernel/c_low_steps.py TRACE.tsv HELPER.log

The helper fills each new volume's log to within one 4 KiB write of the
high mark (BLK_PREFILL_TO_HIGH, line "prefill N: set: ..." with
checkpoint_steps 0), so the case's first call that adds to the log
(write, discard, write-zeroes) takes it over the mark, and the daemon's
rule takes a step before it executes the next request. The helper then
writes each server's step count and gate count when it goes ("steps N
kill" and "steps N destroy").

A volume is put in one of these classes:

  inside      the pre-fill was set, the case made a call after its first
              call that adds to the log, and its servers took steps
  no_write    the case made no call that adds to the log: nothing takes
              the log over the mark, and no step can fall inside it
  last_write  its first call that adds to the log was its last call
  failed_first  the volume's log was failed (the trace's "fail") before
              its first call that adds to the log: the server has
              exited, the calls go to a deleted device, and no step can
              come (programs of prop_failures_match_model)
  not_set     the pre-fill did not leave the log just below the mark
  no_step     set, a call after the first write, and no step counted

Printed: one line `c_low counts inside=I no_write=W last_write=L
failed_first=F not_set=S no_step=N with_gate=G volumes=V verdict=OK|TOO_MANY`, then one
line per volume of the last three classes. TOO_MANY when the volumes
that could hold a step and did not (not_set + no_step) are more than a
tenth of those that could (inside + not_set + no_step), or when none
could: then the pass did not show what the pin promises, which is the
rig's error and not a verdict on the server.
"""
import re
import sys

ADDS_TO_LOG = {'write', 'discard', 'write_zeroes'}


def classify(trace_lines, helper_lines):
    seen, first, after, failed_first = set(), {}, {}, set()
    for line in trace_lines:
        f = line.rstrip('\n').split('\t')
        if len(f) < 3:
            continue
        v = f[1]
        if f[2] == 'open':
            seen.add(v)
        elif f[2] == 'fail' and v not in first:
            failed_first.add(v)
        elif f[2] == 'submit' and len(f) > 4:
            seen.add(v)
            if v in first:
                after[v] = after.get(v, 0) + 1
            elif f[4] in ADDS_TO_LOG:
                first[v] = True
    prefill, steps, gate = {}, {}, {}
    for line in helper_lines:
        m = re.match(r'prefill (\d+): (set|NOT SET)', line)
        if m:
            prefill[m.group(1)] = m.group(2) == 'set'
            continue
        m = re.match(r'steps (\d+) (\w+): checkpoint_steps (\S+) gate (\S+)', line)
        if m:
            v = m.group(1)
            if m.group(3).isdigit():
                steps[v] = steps.get(v, 0) + int(m.group(3))
            if m.group(4).isdigit():
                gate[v] = gate.get(v, 0) + int(m.group(4))
    classes = {}
    for v in seen:
        if v not in first:
            classes[v] = 'no_write'
        elif v in failed_first:
            classes[v] = 'failed_first'
        elif not after.get(v):
            classes[v] = 'last_write'
        elif not prefill.get(v, False):
            classes[v] = 'not_set'
        elif steps.get(v, 0) > 0:
            classes[v] = 'inside'
        else:
            classes[v] = 'no_step'
    return classes, steps, gate


def report(classes, steps, gate):
    count = {k: 0 for k in ('inside', 'no_write', 'last_write', 'failed_first', 'not_set', 'no_step')}
    for c in classes.values():
        count[c] += 1
    could = count['inside'] + count['not_set'] + count['no_step']
    missed = count['not_set'] + count['no_step']
    verdict = 'TOO_MANY' if could == 0 or missed * 10 > could else 'OK'
    with_gate = sum(1 for v, c in classes.items() if c == 'inside' and gate.get(v, 0) > 0)
    out = ['c_low counts ' + ' '.join(f'{k}={n}' for k, n in count.items())
           + f' with_gate={with_gate} volumes={len(classes)} steps={sum(steps.get(v, 0) for v in classes)} verdict={verdict}']
    for v in sorted(classes, key=int):
        if classes[v] != 'inside':
            out.append(f'c_low volume {v} {classes[v]}')
    return out


def main():
    with open(sys.argv[1]) as t, open(sys.argv[2]) as h:
        for line in report(*classify(t, h)):
            print(line)


if __name__ == '__main__':
    main()
