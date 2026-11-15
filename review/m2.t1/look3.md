## Next look at #94 (M2.T1), head 8e2f620: 80f8b44 and 8e2f620

**Nothing blocks the merge.** The two fixes for Codex's findings are right, and their tests would fail if either were undone. One nit is below; it does not block.

- The cut rig on b3b724b ([43654937146](https://github.com/skyzfs/skyzfs/actions/runs/43654937146)) concluded `success` on all four images.
- The run on this head ([43658594842](https://github.com/skyzfs/skyzfs/actions/runs/43658594842)) was still recording when I wrote this. It is the first in which the judge needs zdb's root block pointer on every image.

**How I checked.**
- Both test files pass: 32 and 26 tests.
- `bash -n` passes on `cut-record.sh`. There is no shellcheck here; CI's lint runs it.
- I planted eight changes in the new code (`review/m2.t1/plants3.py`), described below.
- I ran the recording loop, copied verbatim from the script, with stub recordings.

**The root block pointer.**
- `root_of` reads the right words of a `blkptr_t`:
  - the vdev from `dva_word[0] >> 32`;
  - the offset from `dva_word[1]`, without the gang bit, `<< SPA_MINBLOCKSHIFT`;
  - the checksum from words 12 to 15.
- `zdb_root` reads the same values from `snprintf_blkptr`'s `DVA[0]=<vdev:offset:asize>` and `cksum=`.
- The two samples are verbatim from 2.4.1 and 2.2.2, and they decode to the record's words.
- Caught: dropping the root from either comparison in `judge` (the one with the rings' best, and the one at `zdb -t`), reading the checksum one word early, leaving the offset unshifted, dropping the checksum from either side, and reading `DVA[1]`.
- Not caught: making `group()` accept zdb output with no root line. It is equivalent: a missing root never equals the prediction, so the cut still fails, as `opens` instead of `group`.

**errexit in each recording.** Codex is right: inside the left side of `||`, bash ignores `set -e` in everything the subshell runs. I ran the loop with stub recordings:
- a failed command, or a failed pipeline under `pipefail`, now stops the recording, writes a `rig error` line and counts 2;
- a `rig_error` writes one line, not two;
- a verdict of 1 stays 1;
- the next recording still runs.

**Nit, not blocking: an inherited exit status.** The subshell ends with `exit "$WORST"`, and `WORST` includes the recordings before it. In the default mode this is harmless, because the `checked` line is found. With `CUT_CHECK=none` (the workflow never sets it), a good recording after a rig error exits 2. It then has only a `recorded` line, so it gets a false `rig error: a command failed (status 2)`. My stub run of `rigerr ok` wrote exactly that line for `ok`. Fix: end the subshell with `exit 0`, or reset `WORST=0` at its start.

**80f8b44.** The `Loaded` sample is now verbatim, with the tryimport's notes naming the pool. Its comment gives the real reason the last note counts.
