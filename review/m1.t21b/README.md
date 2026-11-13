# M1.T21b (skyzfs/skyzfs#50): the kernel rigs on arm64

`skyzfs-testing-m1.t21b` at a3035b0 into `zfs-topology`. Three workflow files,
20 lines: an `images` dispatch input on the control, daemon and replay rigs;
`ubuntu-26.04-arm` in the pull-request matrix of the control and daemon rigs;
24.04-arm and the replay rig on arm64 by dispatch. Posted as
https://github.com/skyzfs/skyzfs/pull/50#issuecomment-6845117648.

**Verdict: can merge, nothing blocks.**

- `fromJSON(inputs.images || '[...]')` on a pull request: seen working in the
  pull request's own runs (control 43372299321 with three images, the arm job
  green; daemon 43372299265 with six jobs; replay 43372299470 with two).
- "No line differs between the architectures": `table-cmp.py DIR a26d a24d
  x26d x24d` (the judge's table in `summary.md`) and `results-cmp.py` (the
  verdict of every `pin_`, `ctl_` and case line of `results.tsv`) over the four
  arm64 jobs of run 43365073643 (a3035b0) and the four amd64 jobs of run
  43359257961 (befb626, same rig scripts, both servers, in place). Daemon: 78
  rows, PASS 35, NA 13, FAIL 4, WAITING 1, WEAKER 13, NOT_EXERCISED 12 on all
  four; stand-in: PASS 35, WAITING 13, FAIL 5, WEAKER 15, NOT_EXERCISED 10. No
  row differs.
- Points, none blocking: (1) "userland older than its module" is as true of
  amd64 `ubuntu-24.04` (`zfs-2.2.2-0ubuntu9.5 zfs-kmod-2.4.1-1ubuntu5.1`),
  which every pull request runs; the missing mirror list is the reason that
  holds. (2) Take-over times: amd64 24.04 264 to 1572 ms, 26.04 281 to 288;
  arm 26.04 276 to 1824 (13 of 20 above 1 s), arm 24.04 264 to 1787 (10 of 20);
  the daemon's line puts it in "recovery" (187 ms against 1611 ms on an empty
  log). (3) A mistyped label in `images` waits for a runner.
- For the lead: the cost (about 28 minutes per daemon-rig run, about 65 where
  the rig changes), and merging #48 before #50 because of the arm fallback.

## Second look, at 50d19ba

Posted as https://github.com/skyzfs/skyzfs/pull/50#issuecomment-6845159752.
**Can merge, nothing found.** Two workflow comments changed: the reason for
24.04-arm by dispatch is the missing mirror list only. The take-over figures
are corrected in the description; the label check is declined (by dispatch
only), accepted.
