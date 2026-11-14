# M1.3ag (#81): the control program answers a flush whatever its sector reads

- `review-posted.md`: the review posted on #81 at head a0318b6
  (https://github.com/skyzfs/skyzfs/pull/81#issuecomment-6873851036).
  Verdict: nothing blocks; can merge once CI, the daemon rig and the
  replay rig's daemon jobs are green.
- `gates81.sh`, `gates81.txt`: fmt, clippy and tests of the two changed
  crates, plain and with `control-test-program`.
- `plants81.py`, `plants81.txt`: C1-C3 in the program, D1-D3 in the
  translator, and C0 (the old product computed before the match), run
  after the script. All caught. D1-D3 leave a
  `crates/skyzfs-ublk/proptest-regressions/` directory behind, removed
  after the run.
- The flush's sector, measured: dio-split run 43455086383 (artifact
  `dio-split-ubuntu-26.04`, Ubuntu 26.04.1, kernel 7.0.0-1019-azure),
  the daemon's request logs of check c's passes 1 and 3: 330 flushes,
  all with sector 18446744073709551615 and 0 sectors; 321 answers, all
  0. Counted with awk over the `recv` and `commit` lines (columns 5, 6,
  7 and 8).
- Next look at 3ece817 (comments only; no line outside a comment
  differs from a0318b6): the measurement is cited with its scope, and
  the pump test's comment names the refusals right. Verdict unchanged
  (https://github.com/skyzfs/skyzfs/pull/81#issuecomment-6873881733).
