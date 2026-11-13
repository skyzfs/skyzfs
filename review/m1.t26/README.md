# M1.T26 (#56): check g pins the failure's reason; four rig fixes

Reviewed at 602f837, base `zfs-topology` aa16288 (#55 merged). Comment:
https://github.com/skyzfs/skyzfs/pull/56#issuecomment-6852014534

Verdict: two findings block, both in how the new pin is wired into the
rig. The pin's reading, `serve_rc`, the fifo case and the log-store
observation are sound.

## The blocking findings

1. **`G_PINS` (`daemon-rig.sh:2617`) lacks the new pin.** The stand-in
   branch, the log-store branch and the no-file-system branch loop over
   that list, so none writes `pin_failure_reason_is_the_images`, and
   the judge exits 2 with "no result". Check g is in the default
   checks and every rig pull request runs the stand-in. The new unit
   test reads `OBSERVED` only, so 55 tests pass.
2. **A missing line is an ERROR of the pin, and the judge exits 2 on
   any ERROR.** A line is missing when the daemon did what check g is
   for (the start served; the serve did not fail). On the files of the
   negative control (run 43397129438, daemon cd0e763) the pin says
   ERROR, and the judge on that run's results goes from exit 1 with
   three lines that differ (base) to exit 2, "the rig failed". With a
   missing line counted as a FAIL it would be exit 1 with four lines.

## What is here

- `reason-harness.sh RIG_TREE reason DIR`: cuts `check_g_reason` out
  of the rig at RIG_TREE and runs it on an artifact's files.
  `reason-harness.sh RIG_TREE branch reference|log|nofs`: runs the
  rig's block that writes check g's lines without running it, then the
  judge on them with `--checks g`.
- For finding 2's table: copy the negative control's `results.tsv`,
  append the pin's line (and `obs_g_reads_while_failing`, which that
  run is too old to have) and run the judge at the head and at the
  base (`git show aa16288:tests/kernel/daemon_check.py`).

## Run here

- The pin on three real artifacts: 43413318350 (754a15a) PASS;
  43400705709 (#55 at 6355607) FAIL 2 of 4; 43397129438 ERROR.
- `serve_rc`: 652 ms for a status 0.6 s late, 8 ms present, 5381 ms
  never. No `cat` of the file is left; `serve()` empties it per start.
- `test_zfs_stuck.sh`: 8 ok in 7 s. With `lim 5` taken off the stack
  read in a copy, it fails after 31 s with exit 1 and leaves no reader;
  the base's test hangs on the same copy.
- `test_daemon_check.py`: 55 pass; with the reads observation taken out
  of the log-store branch the new test fails.

## Notes

- g.1 reaches the serving loop's call site in `serve.rs` (its line has
  "N requests the daemon held were answered", printed only there); the
  site for a failure that arrives with the queue's end (`serve.rs:1125`)
  is reached by neither g.1 nor g.3.
- The old waiter's status can land after the next `serve()` emptied the
  file (reading only): a stale 137 for the new serve.

## Traps met

- An old run's `results.tsv` judged by a newer judge has errors of its
  own (observations added since): add them before comparing exits, or
  the comparison shows nothing.
- `rec` in the rig also prints to stdout; a harness that evals it shows
  every line twice.


## Second look, at 36f3e7d

Comment: https://github.com/skyzfs/skyzfs/pull/56#issuecomment-6852150598

Both blocking findings are fixed; nothing new. It can merge once the
daemon rig is green on this head (run 43420602078), which is the only
run of the helper's new status file.

- `G_PINS` has the pin; `test_every_pin_of_check_g_is_in_g_pins` fails
  when it is taken out again (56 tests as it is). The stand-in's and
  the log store's lines now pass the judge with exit 0.
- A missing line is a FAIL. On the negative control's files the pin
  says FAIL, and the judge on that run's results exits 1 with four
  lines that differ. The workflow's header names the four.
- The status file now holds "PID STATUS" and `serve_rc` takes only the
  line of the pid in `$DIR.serve.pid`. Run here with the waiter's
  program cut out of the helper: exit 3 gives 3; SIGKILL gives 137
  after 5 ms; an earlier serve's line alone gives empty after 5.2 s;
  that line and then this serve's gives 3; no file gives empty and
  nothing on stderr. Nothing else in the repository reads the file.
- The watcher looks 6000 times (600 s).

For the lead: the guide (CLAUDE.md, "where three lines must differ")
and the notes (impl-notes line 255) still say three for the negative
control. I first told the lead the guide did not give a count, from the
older copy in my context, and corrected it in the next message.

## Traps met (second look)

- The copy of CLAUDE.md a session starts with can be older than the
  one on `zfs-topology`: grep the branch before saying what the guide
  does or does not say.


## The rig on 36f3e7d (run 43420602078), read once

All five kernel jobs green (daemon on ubuntu-24.04, 26.04 and
26.04-arm; stand-in on 24.04 and 26.04). Two tables read:

- ubuntu-26.04-arm, daemon (kernel 7.0.0-1019-azure): the reason pin is
  PASS, all four lines name the image: its first arm64 run. Check c:
  26 cases as declared (PASS 1, WEAKER 13, NOT_EXERCISED 12), 28 kills
  that found no server (the path through `serve_rc` reading 3), no
  FAILED line in the helper's logs.
- ubuntu-24.04, stand-in: the pin is WAITING. Check c: 26 cases as
  declared (PASS 1, WEAKER 15, NOT_EXERCISED 10), 47 such kills.

So the helper's "PID STATUS" file holds on a kernel on both servers,
and the second look's condition is met. Nothing was added on the pull
request; the lead and testing were told.
