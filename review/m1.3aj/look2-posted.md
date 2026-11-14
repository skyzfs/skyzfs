## Next look at M1.3aj: 758ced6

**Nothing blocks.** Section 5 is fixed as suggested, and both of its orderings are held by tests that fail without the fix. It can merge: CI is green on 758ced6.

I read the commit before your answer. At 758ced6 with rustc 1.100.0:
- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- The tests of `skynbd-nbd`, `skynbd-api` and `skynbd-agent` pass: 38 binaries, 414 passed, 0 failed.
- Each of the two new tests passed 30 of 30 runs.

### 1. `Provisioner::converge_origin`: right

**Under `inner`, the serving map and the volume table agree.**
- Their only writers both run under `inner`: the serve's insert (provision.rs:953) and the teardown's remove (:1183).
- The teardown takes the serving out and retires it under that same lock.
- So a handle found there is live, and stays live through its convergence.

**No lock cycle.**
- **The order is `inner`, then the swap lock**, as in the serve path's reconcile and the teardown's `retire`.
- **When it asks for `inner`, `set_origin` holds neither the swap lock nor the volume table:**
  - the first look's `reconcile` has returned;
  - every reader of the table, including `volume()` (lib.rs:382) and the first look, drops its guard before it asks for `inner`.
- **Nothing that holds the swap lock waits for `inner`.** The sync task and the first look never take it.

**The cost.** The second look holds `inner` through a reconcile, which may open an engine, so every operation of the provisioner waits that long. That is the same as a serve's own reconcile. It only happens when the first look found no live serving.

**The 409 test now runs only the path with no provisioner.** All of api.rs's fixtures have `provisioner: None`.
- **With a provisioner, the 409 is still held** by `a_release_while_a_repoint_is_past_its_row_read_leaves_no_origin_export`: its second look finds no serving.
- **With a provisioner, the state that test builds cannot be seen under `inner`.** That state is an ended term with its handle left in the table. That is why the 409 test still needs `provisioner: None`.

### 2. The ordering (b) test and its hold: right

**The hold is in the right place.**
- It sits after the serve's reconcile and the spawn of its tasks, and before the insert.
- It is compiled only into the crate's unit tests.
- It takes the hold out, so it holds one serve.
- A poisoned lock is recovered, not unwrapped.

**Calling `converge_origin` directly is the right choice.** A POST's `volume()` would wait for `inner` through `serve_cataloged`, and the POST would commit only after the serve, which closes the window. The test's commit is `set_origin`'s three operations.

**A right implementation cannot fail it:**
- **The export is on g0 before the commit.** The serve's own reconcile ran before the hold, on g0.
- **`waited` holds.** The second look cannot take `inner` while the serve is held.
- **After `go`, the export is on g1.** The second look converges the serving it finds.

**The 200 ms only decides whether a plant is caught:**
- the full sync must have read the row before the commit, or it converges the export itself;
- S1 and S3 below would then pass.

### 3. The ordering (a) test's 300 ms: acceptable

**Its order rests on tokio's `Mutex` handing the lock out first come, first served:**
- `retire` is queued on the swap lock before the POST's reconcile;
- the serve is queued on `inner` before the POST's second look.

**Only the serve's step can fail a right build.** If the serve has not reached `inner` within 300 ms, the answer is 409, as you say. That step is a spawn and a few calls, and it passed 30 of 30 here.

**The other branch is held too.** I delayed the start of the teardown by 1500 ms, so that the release reached `retire` after the POST had converged. It passed 10 of 10, with 200 and `snap/g1`, as the test's doc says.

### Planted changes

One at a time in the worktree, each against `skynbd-api`'s unit tests, `tests/origin_serve.rs` and `tests/api.rs`, then restored.

| plant | fails |
|---|---|
| S1: no second look (your Q1) | `a_repoint_that_finds_a_serve_half_done_converges_it_through_the_row`, `a_repoint_that_converges_while_a_serve_is_under_way_converges_that_serve` |
| S2: the second look takes no lock and only re-reads the table (Q2) | the same two |
| S3: the second look finds the serving but does not reconcile (Q3) | `a_repoint_that_finds_a_serve_half_done_converges_it_through_the_row` only, as you say |
| S4: the second look lets `inner` go before it reconciles | nothing |

**S4 is harmless.**
- **The serving is complete when it is found.** So its reconcile converges it, or, if a teardown comes in between, converges nothing under an ended term.
- **What changes is only the answer, not the state.** A teardown in that gap can make the POST answer 200 for a serving that has ended, the same window as section 2 of the review. Holding `inner` is what makes the doc's "stays live through its convergence" true, so keeping it is right.

### CI on 758ced6

**All green:**
- ci.yml's test, unit-arm64, lint, chart-lint and s3-backends;
- coverage;
- e2e-tier-a;
- both k3s smokes.

**Skipped:** `skyzfs-ci.yml`'s jobs, as they should be with the pull request open.

**Codex** has not reviewed since its failure at 8dfe6e1.

---
_Generated by [Claude Code](https://claude.ai/code)_
