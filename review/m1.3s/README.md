# M1.3s (skyzfs/skyzfs#46): the contract's power cut is the volatile-cache disk

`skyzfs-coding` at 4424460 into `zfs-topology` (merge base b93f495, one
commit). Posted as
https://github.com/skyzfs/skyzfs/pull/46#issuecomment-6842779626.

**Verdict: one finding blocks as the change is worded, and needs the lead's
ruling more than code. The model's rule matches §4.1 and the store passes the
suite with real cuts. Eight more findings, none blocking.**

Findings:

1. **Blocks.** A failed volume reopened without losing the kernel's cache does
   not keep what its start shows. With `reopen_failed` as the log store's
   harness has it (`disk.kill()`), `reopened_failed_volume_holds_what_was_flushed`
   fails ("block 4 is Torn"). Probe N: at 62 of 64 seeds a block the reopened
   volume showed is different after a completed flush and a power cut. A
   failed sync leaves sectors clean in the cache and never written; no later
   sync writes them. The change makes the harness cut the power before the
   reopen, and the documentation says R11 holds as for the log store. In the
   daemon the hole is narrow (a volume held at its failure is not served again
   in that boot).
2. At a start, the failure note waits for the marker: `assemble` fails, the
   store is dropped, `Drop` joins the marker's thread, and only then does
   `serve` write the note and delete the device.
3. The marker's thread is started at the moment of failure, on the queue's
   thread, under `mlockall(MCL_FUTURE)`: 2 MiB of stack at the moment memory is
   likely short, and the fallback writes inline.
4. The marker's test asserts the store's state: with `fail()` waiting for the
   thread before it returns, all 18 in-place tests pass.
5. After a cut the model holds a discarded block to one content (probe A:
   accepted before this change, refused now). §4.1 lets the reads differ.
   Removing the `Inconsistent` check fails no test.
6. `SECTOR = 256` in the model passes every test.
7. The harness's `power_cut` as a process loss passes every test.
8. `into_in_place` on a failing disk has no test (probe M: it works at each of
   its three syncs).
9. Smaller: `Failed` also for a helper's end; `serve.rs`'s module documentation
   and the help on exit 3; a model test that does not test the floor; the
   closure in `flushed` (probe B); the log store no longer held to a prefix by
   the suite.

What was run: `plants.sh` (m1 to m6 on the model, h1 to h4 on the harness, i1
to i4 on the marker), the probes in `probes-model.rs` (appended inside the
`tests` module of `model.rs`) and `probes-inplace.rs` (appended to
`inplace/tests.rs`).

| Plant | Result |
|---|---|
| m1 `SECTOR = 256` | all pass |
| m2 no floor in `versions` | 1 test fails |
| m3 what was read is not made durable | 3 fail |
| m4 a second, different read at the read-back accepted | all pass |
| m5 the floor exclusive | 1 fails |
| m6 whole blocks in place of sectors | 1 fails; the in-place suite fails |
| h1 the harness's cut loses nothing | all pass |
| h2 the reopen without a cut | the suite fails (finding 1) |
| h3 no cut points | the suite's test fails |
| h4 garbage in the cut | the suite fails |
| i1 `fail()` waits for the marker's thread | all pass |
| i2 `Drop` does not join | 4 fail |
| i3 the thread does not write | 3 fail |
| i4 the writer dropped before the thread | 3 fail |

Probes: J, the suite against `FLUSH_JOINS_ANY_SYNC`: 0 of 8 seeds on the two
harnesses whose syncs end at once, 3 of 8 on the one with late syncs, through
`prop_failures_match_model`. K, six other seeds with no defect: 18 runs pass.
L, which cases notice: a flush that syncs nothing fails the power-cut cases.

Not done: nothing on a kernel; clippy and the feature builds; the rig jobs of
the pull request had not finished.

## After the first look: the lead's rulings (2026-11-13)

- Finding 1: every start, after its sync, drops the kernel's cache of the
  volume's file, checks that it is gone, and serves only what the disk holds;
  it refuses the volume if it cannot (§4.1, §5.1 at `zfs-topology` 86d2441).
  The suite's reopen goes back to a lost process without a power cut, so the
  case pins it. Still blocks #46 until it is in.
- Finding 5: the model must not be stricter than §4.1 on a discarded block's
  reads; fixed if small, else owed.
- Finding 2: the note before the marker at a start, as while serving.

For the second look:

- How the cache is checked. The daemon runs under `mlockall(MCL_FUTURE)`: a
  check by mapping the image and `mincore` would read the range in and lock
  it. `cachestat` (Linux 6.5) needs no mapping.
- `POSIX_FADV_DONTNEED` is advisory and drops only clean pages nothing maps or
  locks; the check is the only evidence.
- Plants to run: a drop that does nothing (the reopen case must fail); a check
  that always passes (a test of the refusal must fail); `plants.sh` h2 is now
  the harness as it should be.
