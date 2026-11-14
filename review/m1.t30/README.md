# M1.T30 (daemon rig: commands_failed, ext4 data=journal, g.1's premise), pull request 72

Head 1f7f434 on 36daf82. Review posted on 2026-11-14 (issue comment
6868877593): one blocking finding.

- g.1's premise is read from the status counters `writes` and
  `flushes`, which count finished requests only (`Pump::finish`). A
  daemon that holds the rig's write forever after the disk failed is
  alive with neither counter moved, so g.1 reads ERROR (exit 2) where it
  used to FAIL. The status key `held` (the pump's in_flight) and the
  dd's own exit would show the write was handed.
- `daemon_check.py` main: `2 if errors else 1 if differs`.
- Check c's awk matches the request log's columns (record.rs: time,
  pid, ended, tag, command, -, -, result). The `ended` lines and the
  status word came in one commit (06d1683), so the guard is sound.
- `sha256sum -- *` misses dotfiles and directories (tried: a new
  directory's line is an error message the sed does not match).
- Local: test_daemon_check.py 66 passed; test_c_evidence.sh all passed.
- The daemon rig at the head (run 43535492589) was still running when
  the review was posted: its table is to read at the next look.

## Second look, 9987c0a (2026-11-14, issue comment 6868931705)

Testing took the suggested rule: serve_wait first, FAIL on an
unanswered write or `held` > 0, ERROR only when the write could not be
issued. That rule (mine) has a hole: in 9 of 10 runs the device was
already deleted when the rig's write came, so "failed to open" is the
common outcome, and a daemon that fails the volume, deletes the device
and then hangs reads ERROR. Asked for: ERROR only with the counters
unmoved, `held` 0 and no failure line in the serve log. Still blocking,
one condition. gj_digests checked here (names `.hidden`, `sub`,
`log.super`); test_daemon_check.py passes at 9987c0a.
