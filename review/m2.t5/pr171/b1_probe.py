#!/usr/bin/env python3
"""B1 at the judge: R-rewind's output as run 45918311854 printed it, but
with round 2 run to its tick (round 1's publish still under way) and the
SIGTERM landing in round 3, which prints its line as it ends. The rig
stamped two arrivals, as await_rounds does. argv[1]: a worktree of the
pull request's head. Run with python3 -I."""
import sys

sys.path.insert(0, sys.argv[1] + '/tests/kernel')
import agent_check as ac  # noqa: E402

OUT = '''round 1 ok took_ms=10488 late_ms=0 skipped=0 begun=skz-20261127T005349Z under_way=1 synced=6
round 2 ok took_ms=9512 late_ms=488 skipped=0 begun=skz-20261127T005359Z under_way=2 synced=0
round 3 ok took_ms=140 late_ms=10 skipped=0 begun=skz-20261127T005409Z under_way=3 synced=0
'''
ARRIVALS = '1\t1795740839.490843\n2\t1795740849.010000\n'
outs = ac.read_outputs(OUT)
rounds = outs['rounds']
print('judge_rounds:', ac.judge_rounds(rounds, ac.read_arrivals(ARRIVALS), 2))
print('judge_synced:', ac.judge_synced(rounds, outs['published'], ''))
print('ended_within:', ac.ended_within('1795740849.010100', '1795740849.300000'))
