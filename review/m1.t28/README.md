# M1.T28, the rig's half: check c keeps its evidence (pull request #59)

Branch `skyzfs-testing-m1.t28`, base `zfs-topology` at c2169bc, head
9addf69. The review is the comment
https://github.com/skyzfs/skyzfs/pull/59#issuecomment-6859279302.

## What the pull request changes

Check c of the daemon rig (the conformance suite through the block
device) now records, for both of its passes, the daemon's request log
of every volume (`--request-log`, M1.3y), the extents of each volume's
files, and the kernel's block trace (`tests/kernel/blktrace.sh`). The
artifact keeps them for a pass in which a case failed or the tool did
not end. udev's watch on ublk devices is turned off for the check by a
rule in `/etc/udev/rules.d`. A new observation, `obs_c_evidence`, says
what was taken; with no block trace it says "not run", which the judge
makes the rig's error. The diagnostic that found the cause of #58's rig
failure (`skyzfs-dio-split.yml` and its two scripts) is added too.

## The review

Nothing blocked the merge. Two small defects, one question for the lead.

1. `blktrace_off` does not leave ftrace as found. Run on this
   session's own machine (kernel 6.18, 4 CPUs; tracefs can be mounted
   here, though nothing else of the kernel's can be used):
   `ftrace-state.txt`. `tracing_on` is left 0 (found 1) and the buffer
   stays at 64 MiB a CPU (found 1408 kB): 256 MiB that do not come
   back. Clock, tracer and events are set to the boot defaults, not put
   back. No pin reads memory or ftrace, so no verdict changes.
2. The rig's error for a missing trace is decided from
   `blktrace_ready` (do the event directories exist) and not from what
   the pass left: a pass with no trace line, or with no request log
   from a server that keeps one, passes the judge. The severity itself
   (exit 2) is right once it looks at the result: the evidence is needed
   only on a rare failed pass, and its absence found then costs tens of
   runs.
3. `check_c_low` runs the suite a third time after the watch is back
   on, with no evidence. Log store only, frozen.
4. For the lead: with the model's half merged too, no rig reaches the
   case that half is for (a call the old server completed that returns
   after the restart), because what produced it is turned off. A
   control pass with udev's reads provoked on purpose would.

Checked and found right: the keep condition (the tool exits 0 when a
case fails, so the `awk` clause carries a failed pass); the stand-in's
`serve --help` (usage, exit 2, no server started); the rule's effect
(one job's artifact: 61 `add`, 61 `remove` and no `change` event of a
ublk device in each pass, so the zero is a real one).

What stays of udev with the watch off: the probe at each device's
`add` event, about 39 read-ahead requests that include the suite's
window (blocks 16368, 16382, 16383). In the two kept passes of run
43455086383 it was over at least 146 ms before the tool's first
request on each of 110 devices (`add-probe-gap.txt`).

The ten check-c-only jobs (20 passes, all green) do not by their count
show that the watch was the trigger: the same daemon failed 1 of 30
traced passes with the watch on, and against that rate 20 clean passes
come out one time in two. The mechanism shows it: no change event, and
the read that held the page came from `udev-synth`.

The dio-split diagnostic: its probe tested the account the first look
at #58 gave and withdrew, and its texts still state that account as
the question. Its second part (check c's pass many times, several
daemons in turn) is the only thing left that runs the pass with the
watch on.

## The files here

- `plants.sh WORKTREE`, `plants.txt`: seventeen defects planted in a
  copy of `daemon-rig.sh`, against `test_c_evidence.sh` and
  `test_daemon_check.py`. Eleven are caught. Not caught: the watch never
  turned off, the request log not asked for, the trace never started,
  the evidence started after the tool, `blktrace_off` missing from
  `cleanup`, and a missing trace not written as "not run".
- `ftrace-state.sh WORKTREE`, `ftrace-state.txt`: ftrace's state before
  `blktrace_on` and after `blktrace_off` on a real kernel (root, and
  `mount -t tracefs nodev /sys/kernel/tracing` first).
- `as-found-nokernel.sh WORKTREE`, `as-found-nokernel.txt`: the same
  with `BLKTRACE_T` at a directory of plain files, to show that a test
  with no kernel can hold "as found". Its diff of `trace`,
  `set_ftrace_filter` and the clock's list is the plain files', not the
  kernel's.
- `add-probe-gap.awk`, `add-probe-gap.txt`: from a block trace, for
  each new ublk device, udev's first burst of requests and the time to
  the tool's first request.

## For the next review

Tracefs mounts in this session's machine, so a script that only uses
ftrace's files can be run here for real. Unmount it afterwards.

## The second look (head c812539)

The comment is
https://github.com/skyzfs/skyzfs/pull/59#issuecomment-6859925472.
Every finding of the first look was settled in c812539, and nothing new
blocked.

- `blktrace_off` now puts back what `blktrace_on` found. On the same
  kernel the state before and after is equal (`ftrace-state-c812539.txt`).
- The rig's error is decided from what each pass left: no trace line,
  no request log from a server that keeps one, or a change event of a
  ublk device while the watch is meant to be off.
- The watch stays off through `check_c_low`.
- The probe is gone; what is left is `c-repeat-rig.sh` and
  `skyzfs-c-repeat.yml`, with an input for the watch.

Left, small: a buffer found as `7 (expanded: 1408)` (ftrace not yet
used) is put back as 7, and a buffer written as 7 stays 7 when an event
is enabled (measured), where the boot state grows to 1408; and two gaps
in the tests (`plants2.txt`: a `blktrace_on` that assumes `tracing_on`
1; "not run:" dropped from the line).

To revisit when the model's half (#60) is in: a change event of a ublk
device is then harmless and still the rig's error.

`plants2.sh WORKTREE` holds the plants of this look.
