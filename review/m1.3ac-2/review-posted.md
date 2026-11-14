## Review of M1.3ac part 2 at 14df6f7

**One finding blocks, and it is a missing test, not a defect.** The guard is right and in the right place. No path I found commits a tag twice, or commits one with no request out. But two new lines on the failure path have no test. With either one undone, the pump ends at the next tag the driver hands back after the failure (finding 1). A short test, below, holds both. `QueueDown` is the right response to a driver that breaks its rule. One remark on that is for the lead.

I read `e82fe77` (the merges bring only what `zfs-topology` has) before the description. At the head, with rustc 1.100.0, these all pass:
- fmt;
- clippy `-D warnings` (skyzfs-ublk, the workspace with the test hooks, and with `control-test-program`);
- the tests of skyzfs-ublk and skyzfsd (with the hooks).

I planted eight defects (table below).

### Blocking

**1. A tag answered after the failure and then handed over again: nothing tests that the pump takes it.**
- **The two lines.** With the new `Waiting` state, the failure path must put a slot back to `Idle` once its request is answered:
  - `fail()` does it for a request that waits in the backlog (`pump.rs:543-545`: the `replace` turns `Waiting` into `Idle`, and the backlog loop commits it);
  - the failed turn does it at `pump.rs:603`.
- **Undo either and every test passes:**
  - P2: the failed turn leaves the slot `Waiting`;
  - P4: `fail()` leaves a waiting slot `Waiting`.
- **Scenario.** The volume fails while tag 1 waits in the backlog. `fail()` answers it with EIO. The driver, which may now reuse tag 1, hands it over again. `take_over` finds `Waiting` and the pump ends with `QueueDown` instead of answering it. The same happens to tag 2, answered by a failed turn, when it comes back.
- **What that costs.** In the daemon the failure is still reported, since `serve` reads the flag and not the ending. But every request after that point is left to the device's delete or to a take-over, and not answered by the pump. The failure line's counts change with it.
- **A test that holds both** (I ran it at 14df6f7: it passes unplanted and fails with P2 and with P4, on `left: Some(QueueDown)`):

```rust
/// After the failure, a tag answered with the plain I/O error is
/// handed over again, as the driver may once its commit is in: it is
/// answered again, and the pump goes on. Tag 1 waits in the backlog at
/// the failure (answered by `fail`); tag 2 arrives after it (answered
/// by the failed turn).
#[test]
fn a_tag_answered_after_the_failure_is_taken_again() {
    let eio = -crate::translate::EIO;
    let mut rig = Rig::new(0, 8, 1);
    assert_eq!(rig.fetch(&[write(0, 0)]), None);
    assert_eq!(rig.fetch(&[write(1, 1)]), None, "tag 1 waits");
    assert_eq!(rig.pump.fail(), 2, "tag 0 in the target, tag 1 in the backlog");
    assert_eq!(rig.fetch(&[write(2, 2)]), None, "tag 2 after the failure");
    assert_eq!(rig.fetch(&[write(1, 3), write(2, 4)]), None, "tags 1 and 2 again");
    let after: Vec<_> = rig.committed().into_iter().filter(|&(t, _)| t != 0).collect();
    assert_eq!(after, vec![(1, eio), (2, eio), (1, eio), (2, eio)]);
}
```

### What you asked me to judge

**Does any path still commit a tag twice, or one with no request out?** No, by reading and by the fake queue's rule. A tag's slot goes `Idle` → `Waiting` (hand-over) → `Running`/`Finished` (submit) → `Idle`, and it is committed once, in the same step that sets it `Idle`.
- **The turn.** `take_over` runs before anything of the wait is backlogged. It stops at the first tag that is out (`Waiting`, `Running`, `WantsFlush`, `Flushing`, `Finished`), before any commit. `submit` keeps its range check only as a fallback.
- **The failed turn.** It takes over first and only then commits, once per tag; P8, which commits before taking over, fails the new test. Two copies of a tag in one wait stop it, as the third case of your test shows.
- **`fail()`.**
  - It commits each finished request once and sets it `Idle`.
  - It commits each slot in the target or in its flush once.
  - It commits each backlog entry once. Its `Waiting` slot is skipped by the slot loop: P3, which commits it from both places, fails three tests.
- **The stop.** The requests a `Wake::Stop` hands over are neither taken over nor committed. Neither is the backlog, since a stopping pump submits nothing new. That is as before this pull request: nothing is committed twice, and what is not committed stays with the driver.
- **The backlog.** A request popped by `submit_what_fits` and refused by the target leaves the backlog with its slot still `Waiting`, and the pump ends with `TargetGone` or `TargetFault`. If the volume then fails, `fail()` (from `each(.., true)` in `serve`) neither commits it nor finds it in the backlog. It is left uncommitted, as it was before (its slot was `Idle` then). So the comment at `pump.rs:544`, "A waiting request is answered from the backlog below", holds only while the pump runs. That is a word, not a defect.

**Is a `QueueDown` stop the right response to a driver that breaks its rule?** Yes.
- **Why not answer.** Once the driver and the pump disagree about a tag, any commit on it may answer a request other than the one the pump means: a write acknowledged that was not written, or one request's data given to another. Not answering loses nothing durable. What is not committed stays with the driver, and a take-over gets it again.
- **Why not fail the volume.** That would ask for an acknowledgement of a loss that did not happen.
- **For the lead.** The daemon then ends with "the queue of device N ended: QueueDown", the same words as a device deleted under it. When the stop on a refused commit comes, with the reason of its own that the lead decided on #64, a rule broken at the hand-over should get that reason too. Otherwise an operator reads a kernel fault as an ordinary end.

### Planted defects

`cargo test -p skyzfs-ublk --lib pump`, one plant at a time:

| plant | result |
|---|---|
| P1: `take_over` lets a tag the queue does not have through | nothing fails (suggestion 1) |
| **P2: the failed turn leaves the slot `Waiting` after its commit** | **nothing fails** (finding 1) |
| P3: `fail()` commits a waiting request from its slot and from the backlog | 3 tests fail |
| **P4: `fail()` leaves a waiting slot `Waiting`** | **nothing fails** (finding 1) |
| P5: the turn does not take over (the failed turn does) | 2 tests fail, the new one among them |
| P6: `commit_finished` leaves the slot `Finished` | the new test fails |
| P7: the fake queue records no broken rule | the new test fails: its own check that `GUARD_AT_SUBMIT` is caught by the rule |
| P8: the failed turn commits before it takes over | the new test fails |

`GUARD_AT_SUBMIT` and `TAG_REUSED` are asserted in the new test.

### Suggestions (not blocking)

1. **P1.** In the turn, `submit`'s fallback still ends the pump on a tag the queue does not have, but only after the requests before it in that wait were submitted. In the failed turn nothing else stops it, so without `take_over`'s range check the tag would be answered with EIO. A line in the new test would hold it: after `fail()`, a tag at `depth` ends the pump and nothing is committed.
2. **`caught` now accepts a broken rule as "noticed".** That is right for a defect whose only symptom is a second commit. It also means that a plant whose scenario stopped failing would still pass on the rule alone. So when a plant's test changes, it is worth checking which of the two noticed it.

### CI

When I wrote this: lint, e2e and the control rig on three images were green on 14df6f7. ci.yml's tests, coverage, the daemon rig and the replay rig were still running.

---
_Generated by [Claude Code](https://claude.ai/code)_
