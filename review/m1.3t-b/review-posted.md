## Review of M1.3t-b at e41e193

**It can merge once the rigs are green. Nothing blocks.** The table is right: every removed test held the record, its retries or its positions, and no property that still holds lost its test. The ports keep their plants. The kept parts are as you say. One thing is for the lead's notes more than for this code: the test that keeps a failing start's delete under the lock now rests on a failure that no store can give (point 2).

I read `09959c2...e41e193` and listed the removed tests myself before reading the description. Then I ran the gates at the head with rustc 1.100.0, and all were clean:
- `cargo fmt --check`;
- clippy on skyzfs-ublk, skyzfs-local, skyzfsd and skyzfs-ublk-sys, plain, with the test hooks, and with `control-test-program`;
- the tests of skyzfs-ublk and skyzfs-local, and of skyzfsd with its hooks.

### What you asked me to judge

**1. The table.** Right.
- **The counts.** By a static list of `#[test]` functions, 24 go and 3 come, which is your 124 → 103. Each of the 24 is in the table.
- **The five I would have doubted, read against the base:**
  - `every_ending_but_the_orderly_one_commits_nothing_that_waited` is about completions held while `ack_fails`. Without a position nothing is held across a wait: `turn` ends with `commit_finished`, which now commits everything finished.
  - The three delay tests hold only `LateAck`. `a_delayed_commit_is_given_on_when_it_is_due_and_the_wait_is_woken` and `a_stop_gives_on_what_waits_and_delays_nothing_more` stay as they were.
  - `a_start_that_waited_for_the_lock_opens_with_the_position_stored_by_then`: the read under the lock that remains is the failure note's, and it is held. Plant L1 below (the note read before the lock) fails 29 lifecycle tests.
- **What is named as holding a property now exists at the head.** That is `a_target_that_is_gone_ends_the_pump_and_commits_nothing`, the two stop tests, the note's boot test and the scenarios. `FAILURE_AT_TOP` replaces `ACK_AT_TOP` on the rule that `ACK_AT_TOP` pinned.
- **`FAILED_UNSTORED_AS_IS` going is right.** `fail()` still answers what is in `finished` with the error. But `finished` is empty between turns now, so no test can reach it, and a plant there would be inert.

**2. The ported lifecycle tests, and the one-shot marker.** The ports are right and keep their plants. The one-shot marker deserves a word in their docs, and in the notes.
- **The ports.** `FAILED_ANY_BOOT`, `FAILURE_NOTE_AFTER_DELETE` and `UNLOCK_BEFORE_FAILED` are still caught, by the tests' own `holds`. Dropping the two cells of `a_start_notes_its_failure` is covered by `a_marker_and_no_record_of_this_boot`. The new check in the acknowledgement tests (no note, so no marker) is sound.
- **What the one-shot marker stands for.** It stands in for the failed recovery sync, which was a failure that a later open does not repeat. No failure at a start does that any more:
  - the marker lasts until an acknowledgement;
  - `OpenError::Other` stops the daemon without a delete.
- **Experiments in `a_daemon_starts_during_a_failing_start`:**

  | | `UNLOCK_BEFORE_FAILED` |
  |---|---|
  | as committed (one-shot marker) | noticed |
  | E1: a lasting marker (`world.marked = true`) | **not noticed** |
  | E2: a lasting marker, and the intruding start passes `--acknowledge-failure` | **not noticed** |

  In E2 the intruder either fails too (marker, and a record of this boot names a device, so it cannot acknowledge) or is refused by the note. It never serves, so there is nothing for an early unlock to take from it.
- **What follows.** With today's store, a start that deletes outside the lock harms no one in these scenarios. The rule is defence for a failure that may pass, should one come back.
  - I would keep the rule and the tests.
  - I would say in the docs of the two tests that the one-shot marker stands for "a failure the next open does not repeat", which no store gives since M1.3t-b, rather than for the store's marker.
  - It is for the lead whether the notes should say that "delete under the lock" is now a guard and not a fix.

**3. The kept parts.** Right, with two remarks for the lead.
- Nothing in `crates/` or `bins/` reads, writes or removes `acknowledged` now (grep). Reason codes 1, 2, 3 and 5 still decode: `records/tests.rs` writes and reads `Short` and `RecoveryFailed`, and checks codes 1 and 7 by number.
- **`unstored` will be read by less than the description says.** Its reason for keeping the field is that the daemon rig reads it (`pin_unstored`), but #73 removes `pin_unstored` and makes the replay rig and the benchmark take a status without log keys. After both merge, `unstored` is read only by the benchmark's settle, which also takes it absent. Keeping it is harmless, and dropping it is now testing's to agree. The two pull requests point opposite ways only in their words.
- **`position N ms` in the start line now times the failure note's read.** A reader of the start line would take it for the position's. It is kept for `replay_daemon.py`, which is right for now; a rename belongs with testing.

### Planted defects

Each alone, `cargo test -p skyzfs-ublk --lib` (D1: `-p skyzfsd --features test-hooks --lib`):

| plant | result |
|---|---|
| L1: the failure note read before the lock | 29 lifecycle tests fail |
| L2: an acknowledged start keeps the failure note | 3 fail, among them `an_acknowledgement_killed_at_any_step_is_asked_for_again` |
| L3: an acknowledging start opens without the acknowledgement | 7 fail |
| D1: the delayed queue's `flush` sends without giving on what waits | nothing fails, and nothing can: `wait` gives everything on when the stop arrives, and later commits pass through. The `release(true)` in `flush` is a guard |

### CI

When I wrote this, the rigs on this head were still running, and Codex had not posted. I will read the daemon rig's tables when they finish.

---
_Generated by [Claude Code](https://claude.ai/code)_
