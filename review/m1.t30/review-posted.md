## Review of M1.T30 at 1f7f434

**It cannot merge yet: one finding blocks (1 below), and it is a few lines.** g.1's new ERROR path does hide a real failure. A daemon that never answers the rig's write after the disk failed used to be a FAIL of g.1. It is now a rig ERROR, exit 2, which the job reports as "not a verdict on the server". The other two things you asked me to judge hold.

I read `36daf82...1f7f434` before the description, checked the request log's format and the status keys in the daemon at the base, and ran `test_daemon_check.py` (66 tests) and `test_c_evidence.sh` at the head: all pass.

### Blocking

**1. g.1: a daemon that holds a write forever after the disk failed now reads as the rig's error.**
- The premise is decided from the status counters `writes` and `flushes`. Both count requests the pump *finished* (`Pump::finish`; status keys `writes` and `flushes` in `skyzfs-ublk/src/status.rs`).
- Scenario:
  1. The disk is switched to `error`, and the rig's own 64 KiB write reaches the daemon.
  2. The daemon's queue thread hangs on it (a sync on the sick disk, a lost completion: the case `serve.rs` names, "a store that hangs before the failure is known hangs as before").
  3. Neither counter moves, and the daemon is alive at the `kill -0`.
  4. `sync_failure_fails_volume` and `sync_failed_volume_refused` read ERROR "the premise of g.1 did not happen", and `failure_reason_is_the_images` reads ERROR too.
  5. `daemon_check.py` returns 2 whenever there is an error (`main`: `2 if errors else 1 if differs`). The job then ends "the rig itself failed; this is not a verdict on the server", even beside a `sync_failure_suspends_pool` FAIL.
- Before this change, the same daemon failed g.1: "serve was still running 60 s after the pool's wait ended".
- The premise was in fact met: the write reached the daemon, and the daemon kept it. The rig's own evidence says so, and the decision ignores it:
  - the `dd` did not end within its limit, and its exit status is in `$rw`;
  - the status key `held` (the pump's `in_flight`: fetched and not committed) would be above 0.
- **A fix, in the spirit of the change:**
  - Wait for the serve first (`serve_wait 60`), as before. A daemon that then exits failed with its line passes, whatever the counters said.
  - Only if it is still serving, decide from what was *handed*:
    - the rig's write ended neither OK nor with an error within its limit, or `held` > 0: FAIL, a request held on a failed disk;
    - the write was answered, but the volume did not fail: FAIL;
    - the write could not be issued at all (the device could not be opened): ERROR.
- Waiting first also removes a false ERROR that the current order allows. The status file is written on an interval (the rig's own helper above `pin_commands_failed` says so), and a daemon in its exit path is still alive for `kill -0`. If the rig's write was the only one after the switch and the status file is a little behind, a sound daemon reads as "premise missed". I have not seen this happen. It is a matter of the order.
- How sure: certain of the code path, by reading. Not run (no kernel here).

### What you asked me to judge

**(1) Check c's count of `ended` lines, and the guard against a build from before M1.3aa.** Right.
- The awk's columns match the request log: time, pid, `ended`, tag, command, `-`, `-`, result (`record.rs` at the base). `$8 == -19` is the driver's abort (`UBLK_IO_RES_ABORT`, `-ENODEV`).
- The glob reads exactly the logs the pass copied (`$OUT/$tag-c*.requests.tsv`, tags `c` and `cd`). It is read only when the pass left one.
- The guard is sound: the `ended` lines and the status word `commands_failed` came in the same commit (06d1683, M1.3aa). So a binary without the word writes no `ended` line.
- The tests would catch a break: `test_c_evidence.sh` checks the counts "2 2 1", the first refused command named, and both passes; `CommandsFailed` checks the judging and that each check writes its pin.
- Checks a, b and e read `commands_failed` from the instance serving at the end, and ERROR on an empty value with the status version. Right.

**(2) g.4's rule on the volume's files.** Right, as the lead's rule says.
- `home.img` and `volume` changed or gone: wrong. A failure marker that appears, is rewritten or goes: wrong. Everything else but `status`, and `log.super`/`log.N` until M1.3t: named and wrong.
- `DataJournalFiles` holds each branch of `gj_files_judged`.
- The digests are taken after the serve on `data=ordered` has stopped, and the refused serve's own changes are what is compared.
- **Not blocking:** `sha256sum -- *` sees neither a dotfile nor a new directory. A directory's line is an error message, which the `sed` does not match, so it goes unnamed (tried here: a new `sub/` and `.hidden` beside a rewritten `log.super` named only `log.super`). The daemon makes neither today. Listing every entry (`find . -mindepth 1`) would name them if it ever does.

**(3) g.1's ERROR path cannot hide a real failure.** It can: finding 1. The rig's own write itself is a good change. It makes the premise happen in the run that missed it (43521074465), and the 10 runs show it does no harm.

### Not blocking

- **The ERROR on an old build voids a whole run.** `pin_c_commands_failed_zero` ERROR on a daemon from before M1.3aa makes any dispatch of check c with an older `daemon_ref` exit 2. That fits "not a pass". But comparing an old build on check c's other lines then needs reading past the exit code. NA, with its own line in the table, would say the same without that cost. Your call.
- **g.1 is called a failed sync, but now a failed write may be what fails the volume.** With the rig's write, the volume may fail on a failed *write* (dm-error answers the write before any flush). The pin is `sync_failure_fails_volume`, though the reason it checks ("a write or sync of the volume's image failed") covers both. A word in the pin's text would keep a reader from thinking a sync was shown.

### CI

- The daemon rig at 1f7f434 (run 43535492589) was still running when I wrote this, on the daemon and on the stand-in. So were the replay rig's reference jobs and coverage.
- Every job that had ended at the head was green, the replay rig's daemon jobs among them.
- I will read the daemon rig's table at the next look. Codex has no thread.

---
_Generated by [Claude Code](https://claude.ai/code)_
