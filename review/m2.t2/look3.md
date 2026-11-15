## Next look at #97 (M2.T2), head 0f2f8c7

**N1 to N5 are settled, and the head's run passes on both images. Nothing blocks the merge.** One hole of N1's kind remains in two commands of `snapshot_files`: Codex's finding on this head, which I confirm, and a second that my own N5 note led to. It is a few lines, and I would close it here.

### How I checked

- **The diff.** I read `75bf72b..0f2f8c7` before your answer.
- **The head's run**, [43671277917](https://github.com/skyzfs/skyzfs/actions/runs/43671277917): both `record and check on node B` jobs end with `worst exit 0`. For `life` and `slog`, on both images:
  - the published cut passes all seven stages;
  - `digest` is caught at files, `early` at snapshot with the import and the copies passing, `lacks` at copies, `later` at import, `hole` at files;
  - `early`'s line names the three snapshots the pool does have, so the listing worked and the catch is real.
- **The tests here:** `test_nodeb_check.py` (23) and `cargo test -p skyzfs-nodeb` (9) pass.
- **Planted changes** (`review/m2.t2/plants3.py` on `skyzfs-review`): 19 in the lines that answer N1 to N5. The tests catch 13.
  - Putting back 75bf72b's rule for N1, and for N2, fails `VerifyPaths`, as you say.
  - The six that pass are listed at the end. None needs a change.

### What remains: an error of the rig in the listing or the mount still counts as a catch

(`nodeb_check.py:334` and `:341`. Confidence: high; `review/m2.t2/verify_paths2.py` shows both with no kernel.)

- **The listing** (Codex's finding). The exit code of `zfs list` is not looked at, and whatever it printed is read as snapshot names.
  - Scenario: on `early`, `zfs list` times out. The stage reads `snapshot FAILED: no snapshot s24`, the import and the copies pass, and the line reads `CAUGHT_AT_SNAPSHOT`, as expected. `early` is the control the criterion's sentence rests on.
  - On the published cut the same reads as its FAIL, exit 1, where it is the rig's error.
  - This line was there at 5de05aa, the first head I reviewed. I did not see it in either round.
- **The mount.** A `mount` that times out or cannot start is recorded as a failed mount.
  - Scenario: on `lacks`, the mount times out. The line reads `CAUGHT_AT_MOUNT`, which the table now allows.
  - This one is new: at 75bf72b a failed mount was the `snapshot` stage, which the table did not allow for those three. My N5 note asked for the stage and did not say this.
- **Fix.** The rule the head already has for `zpool import` and now for `zdb`: an exit code in `RIG_RC` raises, for both commands. For `zfs list` with another exit code other than 0, do not read its output as names: either the rig's error, or a failed stage of its own that `early` does not allow.

### The answers, against the head

| Point | At 0f2f8c7 |
|---|---|
| N1 | Settled. On a control only `rig: zdb died with signal 6` is a catch, which is the replay rig's rule. The plant that puts back 75bf72b's rule fails the test. |
| N2 | Settled. A published cut that failed a stage keeps that verdict, and the copies read "not judged". With no stage failed before, the error stays the rig's. Both plants are caught. |
| N3 | Settled. `control_verdict` takes the stages that must pass, and the test covers the copies failing and the copies never running. |
| N4 | Settled. A ref whose segments alone are gone is tested, and the plant on the read path is caught. Exit 4 is kept for what the bucket holds. The S3 store reports a missing key as `NotFound` (`s3.rs:142`), so the runner reaches the same arm as the tests. |
| N5 | Settled. The import's message keeps its reason: `ctl_later` now reads `cannot import 'skzcut': I/O error ...`. The table is pinned row by row. The `mount` stage is in; see above for its timeout. |
| The other rigs | As you say: green on 5de05aa, cancelled on the two heads since, which change none of their files. |

### The six plants that pass

- **A control that failed a stage, then another error of the rig in the every-copy check.** The head raises; treating it as not judged passes the tests. Either reading is defensible; the head's is the stricter.
- **`main` not giving `control_verdict` the stages that must pass.** The rule has its test; the line that applies it does not, and `early` is clean on the runner.
- **Two in `snapshot_files`,** which only a runner reaches: a failed mount recorded as the snapshot stage, and the import's message cut to its end.
- **Two arms of `about_content`:** `Format` and `BadRange`.
