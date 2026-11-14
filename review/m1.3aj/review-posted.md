## Review of M1.3aj at 8dfe6e1

**Nothing blocks.** The term closes #88's point 4 and its narrower case. Each part of the change is held by a test that fails when the part is removed. It can merge: CI is green on 8dfe6e1, and no kernel rig reads this code.

One finding, low and not blocking (section 5): a POST /origin that converges while a serve is under way on the same server answers 409, though that serve serves the row. It only costs the caller a retry. In one rarer ordering, which I could not force, it can leave the export on the old generation. The lead agreed to the 409 "when no serving is left", so whether to take it now is the lead's call.

I read the commit before the description. At 8dfe6e1 with rustc 1.100.0:
- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets -- -D warnings` passes.
- The tests of `skynbd-nbd`, `skynbd-api` and `skynbd-agent` pass: 38 binaries, 412 passed, 0 failed.

Codex did not review this head: its review failed on a usage limit, so there are no Codex threads to answer.

### 1. Your point 1: nothing swaps or registers `<vol>/origin` outside the lock

| path | how | under the swap lock | reads the term |
|---|---|---|---|
| the sync task, on each `snap/` event | `reconcile` (sync.rs:88) | yes | yes, the serving's |
| `full_sync`, at the start and after a lag | `reconcile` (sync.rs:128) | yes | yes |
| the serve path, first | `reconcile` (provision.rs:846) | yes | yes, the new term |
| the serve path, replayed | `reconcile` (provision.rs:1035) | yes | yes, the existing handle's |
| `set_origin`, both branches | `converge_origin` → `reconcile` (lib.rs:1091, 1128) | yes | yes, the current handle's |
| a release's teardown | `retire` (provision.rs:1117) | yes | ends it |

- **`swap` is private**, and `repoint` is gone. `reconcile_with` reads the term right after it takes the lock (origin.rs:219), before it reads the row.
- **The teardown's prefix loop** (provision.rs:1121-1126) unregisters without the lock. That is fine:
  - it runs after `retire`, under `inner`, so no serving of the volume can be half started;
  - a convergence still under way holds the ended term and registers nothing.
- **Every product site that builds a `VolumeBinding` or `VolumeHandle` carries the serving's term:** provision.rs:848, 878, 914, 1037 and lib.rs:1173. Only tests build one with `Default::default()`.

### 2. Your point 2: `set_origin` converges through the current serving; right

- **The term is read after the convergence** (lib.rs:1170). That is the right place:
  - if it ended during the reconcile, `retire` took the lock after the swap and unregistered what was swapped, so the 409 matches the end state;
  - if it ended before, the reconcile converged nothing.
- **Both halves are held:**
  - converging through the handle the POST began with (P2) fails `a_repoint_that_outlives_its_serving_converges_the_one_after_it`;
  - answering `Some(h)` for an ended term (P7) fails `a_repoint_whose_serving_ended_before_it_converged_answers_409`.
- **One window is left, and it is harmless.** A release between the term check and `origin_served` gives 200 with `served: null`, the answer P7 calls wrong. No caller is hurt by it:
  - the agent's `repoint_origin` discards the answer (http.rs:456-459);
  - the fence reads `served` from a GET.

  No change asked.

### 3. Your point 3: the teardown's wait under `inner` has no cycle and is bounded

- **No caller of `teardown` runs inside a serving's task**, so it never waits for itself. The callers are:
  - `release` (provision.rs:984);
  - the holder change in `serve_cataloged_as` (:1082);
  - the catalog-failure paths of a create (:514, :526);
  - `delete` (:588).
- **No task takes `inner`.**
  - The sync task waits on the refs watch and list, the registry, an engine's open and the swap lock.
  - The rpo task waits on its engine's `RwLock`.
- **An aborted task that is waiting at an await is dropped there.** So the teardown waits only for a poll already running on another worker, and then for `retire`'s swap lock.
- **That lock can be held by an API reconcile**, which waits on the ref store and an engine's open but never on `inner`. The wait is bounded by the store's own limits, as the description says.
- **The order is free.** Moving `retire` before the abort (R8 below) fails nothing, and nothing should fail: a task's reconcile after `retire` reads the ended term, and a task's publish is still waited for before the prefix loop.

### 4. Your point 4: the timed waits only decide whether a plant is caught; right

| test | wait | with a right implementation |
|---|---|---|
| `a_retire_waits_for_a_reconcile_that_holds_the_swap_lock` | 300 ms | `retire` cannot finish while the lock is held |
| `a_reconcile_waiting_for_the_swap_lock_reads_the_term_once_it_has_it` | 200 ms | the term is read after the lock whenever the reconcile gets there; only P6 depends on the reconcile having passed its read in time |
| `a_teardown_waits_for_the_tasks_it_aborted` | 300 ms | the teardown cannot end while the task's poll is blocked |
| `a_release_takes_down_what_a_repoint_of_its_serving_swaps_after_it` | 300 ms + 300 ms | it accepts 200 or 409, and no export is left in either order; the release cannot finish while the lock is held |

The two other origin_serve tests are driven by the ref store's gate, with 5 s limits only for liveness.

### 5. Finding, low, not blocking: a repoint that converges while a serve is under way answers 409

**The scenario.**
1. `lin` is served on this server, with the row on g0. A POST /origin `{"ref": "snap/g1"}` commits g1.
2. At the same time the volume's serving is replaced on this server. That is either:
   - a holder change, where `serve_cataloged_as` runs `teardown` and then `serve` under one `inner` (provision.rs:1082, 1092);
   - or a release followed at once by a serve.
3. The POST's `converge_origin` falls in that window. It either:
   - finds the old handle with its term ended; or
   - finds no handle, between the teardown's `volumes.remove` (:1120) and the new serve's insert (:928).
4. It answers 409: "no origin export is served here; the next serve of the volume serves what the row names".

**What is wrong with that.**
- **(a) The answer is wrong, but the state is right.**
  - The serve under way is on this server. When its reconcile (:846) comes after the commit, it serves g1, possibly before the 409 is even sent.
  - The cost is a retry:
    - the agent's `promote_advance` returns the error at cycle.rs:1998;
    - a stage's `init_origin` fails at nodemech.rs:456 or 487.
  - Both are retried, so this costs a delay, not data.
  - With an epoch, the caller is the old holder, and a refusal suits it. So this matters for POSTs without an epoch.
- **(b) In a rarer ordering, the export is left behind.**
  - **The ordering.**
    1. The new serve's reconcile reads g0 and swaps it.
    2. The POST commits g1, which also unpins g0.
    3. The POST's converge finds no handle, because the serve's insert has not happened yet.
  - **The export can then stay on g0.** Only the new sync task's first full sync re-reads the row (sync.rs:128).
  - **In practice that full sync reads after the commit**, since a refs list and the snapshot registrations come first.
  - **If it read before the commit**, the export would serve g0 while the row names g1 and nothing pins g0 (Invariant D), until the next `snap/` event.
  - **It needs a POST without an epoch.** With an epoch, the commit is fenced once the new serve holds the lease (:704), and the serve takes the lease before its reconcile.
  - **It also needs the serve's task held** between its spawn (:876) and its insert (:928) for longer than a full sync. tokio's `RwLock` queues the POST's read behind the serve's pending write, so I could not force this ordering in a test, and I rate it theoretical.
  - **The base was different here.** It converged through the old handle, which swapped the new serving's export to g1. The base's own defect was the opposite case, which #90 fixes.

**A fix for both.** When `converge_origin` finds no live serving, take the provisioner's lock once and look again, for example with a `Provisioner` method that locks `inner` and clones the current handle.
- **Under `inner`, no serve or teardown is half done.**
  - If a serving is there, converge through it. That also covers a serve that read the row before the commit.
  - If none is there, a later serve reads the row after the commit, and the 409 is exact.
- **One retry is enough.**
- **No lock cycle.** By then `converge_origin` holds no swap lock, since `reconcile` has returned. The release holds `inner` and waits only for the swap lock.

**Testing it.** The ordering in (a) can be driven like `a_release_takes_down_..._after_it`: hold the swap lock, let the POST commit and wait, then run the release and a serve.

### Planted changes

One at a time in the worktree, each against `origin::tests` and `tests/origin.rs` in `skynbd-nbd`, and against `provision::tests`, `tests/origin_serve.rs` and `tests/api.rs` in `skynbd-api`; then restored. P1 to P7 are your plants, done my way.

| plant | fails |
|---|---|
| P1: `reconcile` does not read the term | `a_reconcile_whose_serving_ended_converges_nothing`, `a_reconcile_waiting_for_the_swap_lock_reads_the_term_once_it_has_it`, `a_repoint_whose_serving_ended_before_it_converged_answers_409` |
| P2: `set_origin` converges through the handle it began with | `a_repoint_that_outlives_its_serving_converges_the_one_after_it` |
| P3: `retire` takes no lock | `a_retire_waits_for_a_reconcile_that_holds_the_swap_lock`, `a_release_takes_down_what_a_repoint_of_its_serving_swaps_after_it` |
| P4: the teardown does not wait for its tasks | `a_teardown_waits_for_the_tasks_it_aborted` |
| P5: the teardown does not retire | `a_release_takes_down_what_a_repoint_of_its_serving_swaps_after_it` |
| P6: `reconcile` reads the term before the lock | `a_reconcile_waiting_for_the_swap_lock_reads_the_term_once_it_has_it` |
| P7: `converge_origin` answers `Some(h)` for an ended term | `a_repoint_whose_serving_ended_before_it_converged_answers_409` |
| R7: `retire` ends the term but unregisters nothing | `a_reconcile_whose_serving_ended_converges_nothing` and `a_retire_waits_for_a_reconcile_that_holds_the_swap_lock`; nothing through the API, since the teardown's prefix loop unregisters `<vol>/origin` anyway. Harmless |
| R8: `retire` moved before the abort | nothing, and nothing should (section 3) |

Your table matches mine on P1 to P7.

### CI on 8dfe6e1

**All green:**
- ci.yml's test, unit-arm64, lint, chart-lint and s3-backends;
- coverage;
- e2e-tier-a;
- both k3s smokes;
- `skyzfs-ci.yml`'s test, unit-arm64 and lint, from the branch's push.

---
_Generated by [Claude Code](https://claude.ai/code)_
