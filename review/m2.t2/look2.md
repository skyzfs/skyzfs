## Next look at #97 (M2.T2), head 75bf72b

**B1 is settled: on `slog` the published cut now passes and every control is caught, on both images. Nothing blocks the merge.** Two small defects in the new judging code are worth fixing here, each a line or two: N1, which is also Codex's finding on this head, and N2. N3 to N5 are suggestions.

I wrote this from the code and the head's run, before your answer. The description still names `513207e..5de05aa`, and its line for the run has its placeholder.

### How I checked

- **The diff.** I read `5de05aa..75bf72b` before any summary.
- **The head's run**, [43669957798](https://github.com/skyzfs/skyzfs/actions/runs/43669957798): I read the logs of both `record and check on node B` jobs. Both ended with `worst exit 0`.

  | Line | `life` | `slog` |
  |---|---|---|
  | published cut | PASS, all six stages | PASS, all six stages; import with `-m` |
  | `digest` | caught at files | caught at files |
  | `early` | caught at snapshot; import and copies pass | the same |
  | `lacks` | caught at copies | caught at copies |
  | `later` | caught at import; kind: an uberblock of a later group | the same |
  | `hole` | caught at files | caught at files |

  The stages are the same on ubuntu-26.04 and ubuntu-26.04-arm.
- **The tests here:** `test_nodeb_check.py` (17) and `cargo test -p skyzfs-nodeb` (6) pass.
- **Planted changes** (`review/m2.t2/plants2.py` and `plants_tool2.py` on `skyzfs-review`):
  - 28 in `nodeb_check.py`: the unit tests catch 15. Of the 13 that pass:
    - two cancel each other, as before;
    - two the runner would catch: the `digest` control held against the unchanged digests, and `zdb` without the log device;
    - nine change nothing that a test or this head's run shows: N3, the table's rows in N5, and seven in `main` and `verify`.
  - 8 in the tool: its tests catch 3, the first round's S6 among them. Of the five that pass, two are the first round's that change nothing a pool image shows, and three are N4.
- **The other rigs.** On 5de05aa the cut rig, which I had not seen, passed ([43666257501](https://github.com/skyzfs/skyzfs/actions/runs/43666257501)), and so did the replay, daemon and control rigs. On 75bf72b the cut, replay and daemon rigs' runs were cancelled about 20 seconds after they started. The head changes none of the files they run.

### Two defects, both small

**N1. On a control, any error of the rig in the every-copy check counts as a catch.** (`nodeb_check.py:292`. Confidence: high; `review/m2.t2/verify_paths.py` shows it with no kernel. Codex's finding on this head is the same.)

- **The code.** `not damaged` lets every result that begins `rig:` become a failed `copies` stage: `zdb` timed out or could not start, a list that is not complete, a ganged block, a block on another device.
- **Scenario.** `lacks` is caught at the copies alone, in all four runs. Its import and files pass, and `zdb` times out. The line reads `CAUGHT_AT_COPIES (expected: caught at import or files or copies)`, and the run exits 0. The check that was to catch it never came to an end.
- **The other direction.** The same result on `early` reads "not otherwise clean; DIFFERS", exit 1: a verdict where it is the rig's error.
- **What the replay rig does.** The docstring cites it, and it counts one result alone as a catch: `rig: zdb died with signal 6` (`replay_check.py:619`).
- **Fix.** The same test here. Every other `rig:` result stays the rig's error, on a control too.

**N2. A published cut that fails a stage, and on which `zdb` then aborts, ends as the rig's error.** (`nodeb_check.py:292`. Confidence: high on the path, which the same script shows; medium on how often `zdb` aborts and does not exit 1.)

- **The code.** Every stage now runs, so a cut whose import or files failed goes on to `zdb`. A `rig:` result there raises, and the run ends with `rig_error`, exit 2, and no `nodeb_cut` line. At 5de05aa a failed import ended as the cut's FAIL.
- **Scenario.** In M2.T3 the daemon's cut does not import, and `zdb` aborts on the same damage. The run says "exit 2, not a verdict", and the stage that failed is lost.
- **What reaches it.** Nothing here: with the stand-in the cut is the reference's. In this run's `later`, which fails its import, `zdb` exits 1 ("can't open"), and that path is right.
- **Fix.** Raise only when no stage before has failed. Otherwise record the copies as failed, or as not judged.

### Suggestions

**N3. `early`'s second condition has no test, and no run would notice its loss.** (`nodeb_check.py:205`, `:476`.)

- `control_verdict`'s docstring states the rule, that `early` must be otherwise clean. The function does not apply it; `main` does.
- Dropping the condition passes the unit tests. The runner cannot notice either, since `early` is clean there.
- Suggestion: give `control_verdict` the stages that must pass, and test it: caught at the snapshot with the copies failed differs.

**N4. No test reads a ref whose commit opens and whose data is missing.** (`main.rs:293`, the test at `:521`.)

- I tried the volume with each of its objects missing in turn. A missing segment gives `Unreadable("read at 0: ...")`, as it should.
- The test deletes everything but the refs and the superblock, so the open fails first. Turning a failed read back into the rig's error passes the tests.
- A ref swapped before its segments are there is the likeliest case in M2.T3. A second case that deletes the `/seg/` keys alone would pin it.
- Two smaller points on the same code:
  - the exit codes 3 and 4 have no test: writing 2 for either passes;
  - any error of the store after the ref is found is exit 4, a store that stops answering included. The engine's error says `object not found` where that is the cause, so the tool could keep exit 4 for that.

**N5. Smaller notes.**

- **The import's message loses its reason.** It is cut to its last 300 characters before it is made one line (`nodeb_check.py:313`). On `slog`, `ctl_later` reads `import FAILED: ata loss. Returning the pool ...`, and `I/O error` is gone. Flatten first and keep the first 300.
- **The table's rows for `lacks`, `later` and `hole` are not pinned.** The test pins `early` and `digest` exactly, and the other three only as some stages other than refs and content. Adding `snapshot` to one passes.
- **The stage `snapshot` is two things:** the snapshot is not listed, or its mount failed. Damage that makes the mount fail would read DIFFERS for those three controls.
- **`later` is still the fallback** in all four runs. The line now names the kind, which is what I asked for.

### What the head settles

| Point | At 75bf72b |
|---|---|
| B1 | Settled, as the lead ruled. The stand-in is the log device after the create and the export (`cut-record.sh:344`), beside the image for `zdb` alone. The import takes the image alone, with `-m`. Every block judged is read from the image: a block on another device is the rig's error. `nodeb_zdb_device` says so in the table. |
| S1 | `EXPECTED` names the stages, and `early` requires that the import and every copy pass. The run shows both. N3 is what remains. |
| S2 | `compare` is its own function with tests, and four plants on it are caught. The `digest` control shows it on the runner. |
| S3 | The line names the kind. |
| S4 | Every detail is made one line (`flat`, with a test), and the plants on it are caught. |
| S5 | Exit 4 and `Got::Unreadable`. N4 is what remains. |
| S6 | The test is in, and the plant that reads the head is caught. |
| S7 | A multihost pool ends as the rig's error. The test is ZFS's own: the delay is not 0. |
| S8 | Any exception ends with exit 2. `later_uberblock`'s condition is tested, and its plant is caught. The docstring on `zdb -cc` is right. The abort on a control went wider than I meant: N1. |
| #95's S5 | The fixtures' README says what a refresh can change, and what to do about the test's table. |
