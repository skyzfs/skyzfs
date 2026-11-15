#!/usr/bin/env python3
"""#103 at 718d198: a write-zeroes received while a flush waits, and the content judge.

    zeroes_behind_flush.py KERNEL_TESTS_DIR

The judge places a write-zeroes at its receipt (the request log has no line for when it ran).
The store runs whatever it receives while a flush waits behind that flush (design §5.2, "The
instant is a flush's barrier"; `InPlace::accept`: a request received while anything waits is
put behind it). So a write-zeroes received between a flush's receipt and its barrier takes
effect after the barrier: the cut of that flush holds the block as it was before.

The log here, as the daemon writes one:
  write of block 10 (content A), applied;
  write of block 30 received, not yet applied;
  a flush received: it waits for the write of block 30;
  write-zeroes over block 10 received: it waits behind the flush;
  the write of block 30 applied; the flush's barrier; the cut (number 1).
A right daemon's cut 1 holds block 10 with A and block 30 with its write.
"""
import os
import sys

sys.path.insert(0, sys.argv[1])
import cut_daemon_check as c  # noqa: E402

P = '77'
A, B = 'a' * 16, 'b' * 16
LOG = '\n'.join([
    f'1\t{P}\tstart\t0\t-\t-\t-\t64',
    f'2\t{P}\tcuts\t-\t-\t-\t-\tbegun',
    f'3\t{P}\trecv\t1\twrite\t80\t8\t-',
    f'4\t{P}\tapplied\t1\twrite\t-\t-\t-\t{A}',
    f'5\t{P}\trecv\t2\twrite\t240\t8\t-',
    f'6\t{P}\trecv\t3\tflush\t18446744073709551615\t0\t-',
    f'7\t{P}\trecv\t4\twrite_zeroes\t80\t8\t-',
    f'8\t{P}\tapplied\t2\twrite\t-\t-\t-\t{B}',
    f'9\t{P}\tbarrier\t3\tflush\t-\t-\t-',
    f'10\t{P}\tcut\t3\tflush\titself\t1\t5',
]) + '\n'

inst = c.parse_log(LOG)[0]
records, dcuts = c.records_of(inst['lines'], [])
written = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 2}]
blocks = {1: {10: A, 30: B}}
got = c.held_content(records, dcuts, written, blocks, {10: A, 30: B})
ok, detail = got[0], got[1]
wrong = got[3] if len(got) > 3 else set()
amb = set().union(*(r.get('ambiguous', set()) for r in records))
print('the records, in the judge\'s order:',
      [('write-zeroes' if r.get('op') == 'write_zeroes' else 'barrier' if 'flush' in r['flags'] else
        'flush received' if 'receipt' in r['flags'] else 'write') for r in records])
print(f'held_content for a right daemon: {"PASS" if ok else "not PASS"}: {detail[:260]}')
print(f'blocks held wrong, by the judge: {sorted(wrong)}; blocks it calls not placed for certain: {sorted(amb)}')
print('the line would read:', 'PASS' if ok else ('ERROR (the rig)' if wrong & amb else 'FAIL (a verdict against the daemon)'))
