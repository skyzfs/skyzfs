## Review of M1.3ai at 7d5bdb7

**Nothing blocks.** The lock closes the race I reproduced, and together the two tests hold it. It can merge once CI is green on 7d5bdb7; no kernel rig reads this code.

I read 9afac71 before the description. At 7d5bdb7 with rustc 1.100.0:
- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets -D warnings` passes.
- The tests of `skynbd-nbd`, `skynbd-api` and `skynbd-agent` pass: 38 binaries, 404 passed, 0 failed.
- Each new test passed 50 of 50 runs.
- `origin_pointer_lifecycle` passed 50 of 50 runs pinned to one CPU.

### 1. Every change to `<vol>/origin` goes through the lock (your point 1): right

Every path that swaps, registers or unregisters the origin export:

| path | how | under the lock |
|---|---|---|
| the sync task, on each `snap/` event | `reconcile` (sync.rs:61) | yes |
| `full_sync`, at the start and after a lag | `reconcile` (sync.rs:101) | yes |
| the serve path, first and replayed | `reconcile` (provision.rs:840, 1024) | yes |
| `set_origin`, no-op branch | `reconcile` (lib.rs:1088) | yes |
| `set_origin`, after its commit | `reconcile` (lib.rs:1123) | yes |
| `repoint` | takes the lock; no product caller left | yes |
| a release's teardown | `unregister` (provision.rs:1097) | **no**: your point 4 |

- **`swap` is private,** and both of its callers hold the lock. The two unregisters inside `reconcile` (no row; no generation left) are under it too.
- **The other unregisters touch other exports.** The sync task's (sync.rs:51) and `full_sync`'s drop (sync.rs:96) only touch `<vol>/snap/*` and `<vol>/rpo`.
- **What it buys.** Each swap now goes to what the row names at that moment. The row moves only forward (`repoint_forward` under `MetaCas`). So what is served now moves only forward too.
- **No lock cycle.** A holder of the swap lock waits on the ref store, the registry's map, the export engine's `RwLock` and an engine's open. None of those waits for a swap lock in turn. `volume()` clones the handle and holds nothing.

**Why it matters beyond the test.** The agent's Invariant E fence (`skynbd-agent/src/cycle.rs:2010-2024`) reads `served` from a GET and then discards blocks. On the base, a stale swap could land after the fence had passed. Blocks would then be discarded while the origin served an older generation than the fence saw. This closes that too.

### 2. The unit test's seam and its 500 ms (your point 2): right, with one gap that the API test covers

- **It models the race I reproduced.**
  1. The sync task's reconcile reads g1 before the API's commit.
  2. The API's convergence swaps g2.
  3. The held reconcile swaps g1 back.
- **It fails for the right reason.** With no lock (B1 below), it fails at its last assertion, "the export serves what the row names, not what a reconcile read before it moved".
- **The 500 ms can only let a wrong build pass, never fail a right one.**
  - With the lock, the second reconcile cannot finish while the first is held.
  - Without it, a reconcile slower than 500 ms lets the held one swap g1 over g1, a no-op, and then swaps g2 itself.
- **The gap: the seam sits after the lock.** So a lock taken after the row is read but before the seam (B2a) passes the unit test. That is the fault that matters: a reconcile reads g1, then waits for the lock while the row moves on.
  - **The API test catches it.** The POST reads g1 before the lock and swaps that stale reading once the lock is free. So the pair holds the lock's placement.
  - **Your table's second row** is true for a lock taken after the seam (B2b), not for every placement.
  - **No change needed.** A sentence in the unit test's doc naming the API test as the one that holds the placement would save the next reader this work.

### 3. The API test (your point 3): right

- **It holds both halves.**
  - The POST waits for the lock: B1 and B4 fail it.
  - The POST converges on the row, not on its own target: B3 fails it.
- **Its 300 ms is safe the same way.** With the lock, the POST cannot answer.
- **One shortcut, and it is fine.** The row moves on by a bare `MetaPut` while the `origin` pin stays on g1. No product path makes that state, but the test judges only the swap.

### 4. A release's teardown against a POST in flight (your point 4): your reading is right

- **The gap.**
  - `teardown` (provision.rs:1086-1100) aborts the volume's tasks, drops its handle and unregisters `<name>/*`, without the swap lock.
  - A POST that passed `volume()` before the teardown converges after it. `swap` finds no export and registers `<vol>/origin` again, bound to the released volume.
  - Nothing removes it until a later serve of that volume converges it again, so it stays until that serve or for good.
- **Not new.** The base's `repoint` registered an absent export the same way. #88 only adds the wait for the lock to that window.
- **The lock alone cannot close it.** A convergence that begins after the teardown has to learn that the volume is no longer served. For example, the teardown could take the swap lock to unregister, and `reconcile` could check under it that the volume is still served.
- **A narrower case of the same kind.** `abort` stops the sync task only at its next pending await. A reconcile running on another worker could also register after the unregister loop.
- I agree it is a package of its own.

### Smaller points

- **`repoint` is public with no product caller.** A future caller would swap to a commit it read before taking the lock, which is the race again. Its doc warns. Making it `pub(crate)`, and moving `tests/origin.rs`'s three calls to `reconcile`, would remove the trap. Not for this merge.
- **The response's `ref` is the POST's own target, while `served` follows the row.** Leaving it as it was is fine:
  - the agent's `ControlPlane::repoint_origin` discards the answer (`http.rs:456-459`);
  - the fence reads `served` from a GET;
  - `init_origin` does read `ref` (`nodemech.rs:456, 487`, `handoff.rs:142`). For an initializing POST, the target is the row, or the newest generation when there is no row. It can be older than the row only if another POST moves the row in between, which is the same as on the base.
- **The serve path now waits in turn.** It reconciles while holding the provisioner's `inner` lock, so it now also waits for a reconcile of the same volume by the sync task or the API. That reconcile may include an engine's open and the in-flight reads of `<vol>/origin`. It is bounded, and the work is the same as before, now in turn. No change asked.

### Planted changes

One at a time, each with `origin::tests` in `skynbd-nbd` and the origin tests of `skynbd-api/tests/api.rs`, then restored.

| plant | unit test | API test |
|---|---|---|
| B1: `reconcile` takes no lock | fails | fails |
| B2a: the lock taken after the row is resolved, before the seam | **passes** | fails |
| B2b: the lock taken after the seam, just before the swap | fails | fails |
| B3: `set_origin` swaps to its own target with `repoint` | passes | fails |
| B4: `swap_lock` hands out a fresh lock each time | fails | fails |

**My reproducer** from `review/flaky-origin-pointer/` (skyzfs-review, d67c815):
- **The 100 ms sleep before the swap, under the lock (B5):** the single-shot test passes, 0 of 200 rounds go stale, and both API origin tests pass.
- **The same sleep with the lock also removed (B6):** it still passes, 0 of 200.
  - Now that `set_origin` converges through `reconcile`, the sleep delays both paths alike, and timing keeps their order.
  - So my reproducer can no longer tell whether the lock is there. Your unit test can (B1).

### CI on 7d5bdb7, when I wrote this

- **Green:**
  - ci.yml's lint, chart-lint, s3-backends and unit-arm64;
  - coverage, the workflow where the flake was seen;
  - e2e-tier-a and both k3s smokes;
  - `skyzfs-ci.yml`'s test, unit-arm64 and lint, from the branch's push.
- **Running:** ci.yml's test.
- **Codex:** its code review of 7d5bdb7 has no finding. Its security review was still running.

---
_Generated by [Claude Code](https://claude.ai/code)_
