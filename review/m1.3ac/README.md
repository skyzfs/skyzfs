# M1.3ac, first part (the control program's malformed commit), pull request 71

Head a3e15ad on 36daf82. Review posted on the pull request on
2026-11-14 (issue comment 6868793457): can merge, nothing blocking.

- The refusal of a commit with address 0 was checked in `ublk_drv.c`
  at v6.11 (line 1744, in `__ublk_ch_uring_cmd`), v6.17 and v7.0
  (`ublk_check_commit_and_fetch`), and in libublk 0.4.8
  (`complete_io_cmd_unified`: an empty slice is the address 0; neither
  of its checks refuses it for a queue with no flags).
- The control rig at the head printed
  `control_commands_aborted_at_stop_detail 8` on all three images.
- After SIGTERM the server's count of aborts is 0 by construction: it
  returns at `Woken::Stop` before the driver aborts anything.
- The pump's guard against a tag handed over again is in `submit`,
  not at the hand-over: a re-handed tag that waits in the backlog for
  room gets past it (by reading pump.rs at a3e15ad, lines 727, 835,
  860, 1031). Told to coding and the lead for the second part.
- The red daemon-rig job on 24.04 (132234150172) differed only in
  g.1's three lines: the premise that pull request 72 is for.

## Second look, head 4883c98

Posted on 2026-11-14 (issue comment 6870567715): can merge.
`look2-posted.md`. e556428 returns the library's result from
`commit_with_no_buffer_for_tests`, and the program prints `not sent`
and answers the request; the merge of 81247f2 brings no Rust file.
