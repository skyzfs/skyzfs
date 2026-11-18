# #121, the last look at b189873

Pull request: https://github.com/skyzfs/skyzfs/pull/121. Head b189873, one commit on 75f3227 (the head of `review.md`). I read the commit's diff before the answer (issuecomment-6980352033).

**Verdict: it can merge. No blocking finding.** B1 is fixed, and the test that holds the list fails on a gap. Four small points are left, below.

CI at b189873 when I looked: 24 checks green, 19 still running or queued, none failed; "can merge" assumes they end green. Codex reviewed b189873 and has no finding. In the pull request's own cut-daemon run 44316490941 the plan gave three jobs and the upload job is done and green; the two held cuts' jobs were still running. The head's tests pass here (`test_cut_plan.py` 10, `test_cut_record.sh`).

## B1: fixed

- `closure.py` on `cargo metadata --locked --all-features` at b189873 (`closure-b189873.out`): all 15 workspace crates under `skyzfs-upload`, `skyzfsd` and `skyzfs-nodeb` are in `UPLOAD_PATHS` and in the workflow's `paths`. Nothing is listed that no root is built from, and every `UPLOAD_PATHS` entry is under the workflow's `paths`.
- The judge's imports (`cut_check.py`, `cut_ref.py`, `replay_check.py`, `logwrites.py`) are in both. The eight modules import nothing else from outside the standard library but numpy.

## `closure()` and `imports()` in test_cut_plan.py: do they fail on a gap?

Yes, for what a manifest or an import statement can say. Plants in `plants5.py`: 78 gaps put in one at a time with no cargo on `PATH`, which is how CI runs the test (see point 1): 65 killed, 13 survive, and one of the 13 is right to survive.

| Gap | Plants | Killed | Survive |
|---|---|---|---|
| A path taken off `UPLOAD_PATHS`: each of the 15 crates, the 8 modules, the 6 other entries | 29 | 29 | |
| A crate's line taken off the workflow's `paths`, each of the 15 | 15 | 15 | |
| `tests/kernel/cut*`, `replay_check.py` or `logwrites.py` off the workflow's `paths` | 3 | 3 | |
| Another line off the workflow's `paths` (Y130 to Y134) | 5 | | 5 |
| A crate left out by `!pattern` or `paths-ignore` (Y140, Y141) | 2 | | 2 |
| A new dependency on a workspace crate in neither list (M01 to M10) | 10 | 10 | |
| The same as a dev-dependency (M20) | 1 | | 1, rightly |
| A new `import` of a module in neither list (I01 to I08) | 8 | 8 | |
| The module reached without an `import` statement (I10 to I14) | 5 | | 5 |

What each asked form does:

- **A crate off either list**: killed, for every crate.
- **A dependency** written `{ workspace = true }`, dotted (`x.workspace = true`), as a table of its own, by `path`, by `path` under another name (`package = ...`), as a build dependency, on one target, optional, on node B, or two levels down in `skynbd-tree`: all killed by `test_every_crate_under_the_uploader`. The closure goes by path, so a renamed package makes no difference.
- **A dev-dependency** is not followed, which is right: it is not in the binary. `cargo metadata` agrees.
- **An import** at the top, `from x import`, `import x as y`, two on one line, under `try`, or inside a function (`ast.walk` reaches every node), also in a module reached only through others (`logwrites.py`): all killed.
- **Not seen, as one would expect from the method**: `importlib.import_module`, `__import__`, a script run as a command from Python, and a script that `cut-record.sh` runs or sources. `UPLOAD_SCRIPTS` and `cut-record.sh` itself are a list kept by hand. I checked that nothing of the kind is missing today: `cut-record.sh` names `cut_watch.py`, `cut_check.py` (not used in the upload job, and listed), `cut_hold.py` (excluded with upload, line 182) and `tests/lib/gnu-dd.sh`; `replay_check.py` runs `logwrites.py` as a command, and that file is listed through `cut_check.py`'s import.

`test_the_closure_from_cargo` agrees with `cargo metadata` here, and with my `closure.py`. With cargo on `PATH` the ten dependency plants are killed as well (`plants5-cargo.out`), M07 by both tests.

## Small points, none blocking

1. **The comparison with cargo does not run in CI.** `cargo metadata --offline` fails on a runner that has fetched nothing, and the test skips: the cut-daemon unit job's log says `skipped "cargo metadata: ..."`, and the `lint` job of `ci.yml`, which runs the file before any build, says `OK (skipped=1)`. So CI rests on the manifests' closure alone. That one held every plant above without cargo, so the cover is real; the second test is a local check. Running the file once more after a job's build would make it run.
2. **That test can change `Cargo.lock`.** It calls cargo without `--locked` (test_cut_plan.py:134). With a manifest changed and the lock not yet updated, cargo rewrites `Cargo.lock` in the checkout: it did after ten of the eleven dependency plants here. It is what a build would do next anyway, but a test that writes a tracked file surprises. `--locked` would make it skip there.
3. **Nothing holds that the other `UPLOAD_PATHS` entries start the workflow** (Y130 to Y134): with `Cargo.lock`, `Cargo.toml`, `rust-toolchain.toml`, `tests/lib/**` or the workflow's own line taken off `paths`, every test passes. The new test checks the crates and the modules only. One loop over `UPLOAD_PATHS` with `in_workflow` would hold all of it.
4. **`in_workflow` takes every line as a path that includes** (Y140, Y141): a `!crates/x/**` line or a `paths-ignore` block is not understood. Nobody writes one by accident.

## The recorder's test

R64, R66 and R67 are killed, as the answer says, and so are R60, R62, R65 and a new R68 (group 0 before the first act only): 7 of 7 (`plants5-recorder.out`). The stand-in now records each control's group, and the test reads `guard-controls.txt` as well.

## Files

Under `review/m2.t3-4/` on `skyzfs-review`: `review2.md` (this), `plants5.py`, `plants5-nocargo.out`, `plants5-cargo.out`, `plants5-recorder.out`, `closure-b189873.out` (made with `closure.py`). The first look's files are beside them.
