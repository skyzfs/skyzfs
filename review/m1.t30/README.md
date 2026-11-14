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
