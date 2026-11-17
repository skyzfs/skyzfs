#!/usr/bin/env python3
"""#111 at 8210332: three scenarios on the reference and the judge, with no kernel.

    scenarios.py CHECKOUT

1. The stored mark is cleared by a cut of its own state only (plant R7's
   case, which no test has): group 5's uberblock, a flush received, group
   6's uberblock, the flush's barrier (cut 5), a take-over, a flush.
2. The newest cut on a created device (plant R10's case): a cut of group 5,
   a created device, group 5's uberblock again, a flush.
3. A write in flight at the kill that reached the disk, and a cut in the
   next instance before the kernel's reissue of the write is applied: what
   a right daemon's held cut reads, and what the judge says of it."""
import os
import sys

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests/kernel'))
import cut_daemon_check as c  # noqa: E402
import test_cut_ref as tr  # noqa: E402

run = tr.Instances.run_on
steps = [('ub', 5), ('recv', 'a'), ('ub', 6), ('barrier', 'a'), ('over',), ('recv', 'b'), ('barrier', 'b')]
print('1. ub 5, recv a, ub 6, barrier a, take-over, recv b, barrier b -> cuts (group, flush record):', run(steps))
steps = [('ub', 5), ('recv', 'a'), ('barrier', 'a'), ('created',), ('ub', 5), ('recv', 'b'), ('barrier', 'b')]
print('2. ub 5, recv a, barrier a, created device, ub 5 again, recv b, barrier b -> cuts:', run(steps))

P, P2 = '100', '200'
h0, h1, h2 = 'a' * 16, 'b' * 16, 'c' * 16
log = [
    (P, 'start', '0', '-', '-', '-', '64'), (P, 'cuts', '-', '-', '-', '-', 'begun'),
    (P, 'recv', '1', 'write', '80', '8', '-'), (P, 'applied', '1', 'write', '-', '-', '-', h1),      # block 10
    (P, 'recv', '2', 'flush', '0', '0', '-'), (P, 'barrier', '2', 'flush', '-', '-', '-'),
    (P, 'cut', '2', 'flush', 'itself', '1', '5'),
    # block 10 again, in flight when the daemon is killed: no `applied` line; its direct write reaches the disk
    (P, 'recv', '3', 'write', '80', '8', '-'),
    (P2, 'start', '0', '-', '-', '-', '64'), (P2, 'cuts', '-', '-', '-', '-', 'taken_up'),
    # the kernel hands over a flush before the write (the state kept a mark), and it cuts
    (P2, 'recv', '4', 'flush', '0', '0', '-'), (P2, 'barrier', '4', 'flush', '-', '-', '-'),
    (P2, 'cut', '4', 'flush', 'itself', '2', '6'),
    (P2, 'recv', '3', 'write', '80', '8', '-'), (P2, 'applied', '3', 'write', '-', '-', '-', h2),
]
text = ''.join('\t'.join((str(10 * (k + 1)),) + ln) + '\n' for k, ln in enumerate(log))
records, dcuts = c.records_of(c.log_lines(c.parse_log(text), {P2: True}), [])
written = [{'number': 1, 'group': 5, 'opens': 'itself', 'blocks': 1, 'asked': 75},
           {'number': 2, 'group': 6, 'opens': 'itself', 'blocks': 1, 'asked': 135}]
# A right daemon: cut 1 reads block 10 as the first write left it (from its copy); cut 2's instant is after
# the killed instance's write reached the disk, so cut 2 reads what the disk holds, the second write's bytes
# (design 5.2: "A write that a lost daemon had half applied can be half in a cut ... without harm").
blocks = {1: {10: h1}, 2: {10: h2}}
ok, detail, _, _ = c.held_content(records, dcuts, written, blocks, {10: h2})
print(f'3. held_content: {"PASS" if ok else "ERROR" if ok is None else "FAIL"}: {detail[:230]}')
