Next look at #115 at 813120a (covers 5ed17fe)

**Verdict: it can merge. No finding blocks.** The two commits after fe6970c do what their author says. Every plant they claim is killed here, each by the test named for it. G1 and F1 of my review are closed. I have one finding, and it is not blocking: the new main test for a group synced with no flush kills its plant, but its log is not one that `zfs_nocacheflush` gives with the marks it writes (N1 below).

I read `git diff fe6970c..5ed17fe` (tests only: three test files) and `git diff 5ed17fe..813120a` (the words of one INFO line, one new test, two assertions' strings) before the author's two notes.

## What was asked

### 1. `test_a_group_synced_with_no_flush` (N5): does it model the span fairly?

**It kills N5 and it is not vacuous, but the log is not the one the recording gives.**

What the test does (`test_cut_daemon_main.py:323-366`), by the fixture's own times:

| Time | Line of the fixture `life` | In the test |
|---|---|---|
| 18175 | | the mark `noflush-begin` |
| 20280 | a flush, before group 21's uberblock writes | **kept**, though it lies between the marks |
| 20300 to 20450 | group 21's four uberblock writes | kept |
| 20460 | a flush: cut 11, group 21 | taken out |
| 20485 | | the mark `noflush-end` |
| 21420 | a flush, before group 22's uberblock writes | **taken out**, though it lies after `noflush-end` |
| 21600 | a flush: cut 12, group 22 | kept |

What the recording does: `noflush` writes its mark and then sets `zfs_nocacheflush`; `flushing` clears it and then writes its mark (`cut-record.sh:741-742`). So ZFS sends no flush between the marks and every flush after them. Six real `takeover` recordings say the same (`n5_real.out`, three runs, both images):

- **0 flushes received between the marks**, in all twelve spans.
- Course (b), one group in the span and a take-over: the first flush after `noflush-end` **cuts that group**. Nothing is left out.
- Course (c), three groups in the span and then a created device: none is cut, and all three are left out (479, 483 and 492 in the run on this head).

So in the test, group 21 goes uncut because a flush is missing *after* `noflush-end`. With these marks a real pool sends that flush, the daemon cuts group 21 at it, and no group is left out. I ran both through the program (`n5_model.py`, `n5_model.out`):

| Variant | Result |
|---|---|
| V0, the test's log and marks | exit 0, `obs_noflush_groups` names [21], `sessions` PASS |
| V0n, the same with no marks | exit 1, `sessions` FAIL (the test's second half) |
| V3a, what the pool sends with these marks (the flush at 21420 there), the daemon cutting nothing at it | exit 1, `cuts` FAIL: the reference cuts group 21 there |
| V3b, the same and the daemon cuts group 21 at that flush | exit 0, no `obs_noflush_groups`, 26 cuts |
| V2, no flush between the marks and every flush after: `noflush-end` at 21435, the flush at 20280 gone too | exit 0, [21] left out, `sessions` PASS |
| V4, two groups between the marks (18 and 21), the daemon cutting 21 at the first flush after | exit 0, [18] left out, `sessions` PASS |

**N1 (not blocking).** The test's marks and its missing flushes do not agree: one flush stands between the marks, and one is gone after them (`test_cut_daemon_main.py:338` for `hi`, `:342-344` for `gone`).

- Effect today: none on what the test holds. `noflush_kstats` reads only where the uberblock writes lie against the marks, and nothing in the check holds flushes to the marks. V2 and V4 are logs a pool can give, and they pass the test's own assertions.
- Why it matters: the test's comment says "under zfs_nocacheflush the pool sends none of them", and the description's table says "two flushes gone inside a `noflush` span". One of the two is outside the span. Whoever adds a rule that ties flushes to the marks (for example, a flush received between the marks is the rig's error) will find this test in the way, and it reads as a model of the recording when it is not.
- The change: put `noflush-end` after the last flush taken out (`hi` from that barrier's time) and take the flush before group 21's uberblocks out too. That is V2. V4 is nearer to course (c).
- Certainty: high. Run, on the fixture and on six recordings.

What the test does hold, and that is the claim it was written for:

- With plant N5 (`main` drops the kstats that `noflush_kstats` returns) it fails, here too.
- It is not passing for a trivial reason. Four faults the other way round are killed by it: `main` says nothing of the groups left out (O14), says the line in every recording (O15), leaves every uncut group out with or without marks (O16), holds the sessions to nothing (O17).
- The kstats it is judged against are the whole fixture's, with group 21 in them, as ZFS's own account has it.

### 2. The helper's new hooks, `tamper_starts` and `tamper_marks`

**They are used only where meant, and I do not see how they hide a bug.** `test_cut_daemon_main.py:31`, `:56-57`, `:95-96`.

- Each has one use: `tamper_starts` in `test_a_failed_instance_line_is_a_verdict` (`:318`), `tamper_marks` in `test_a_group_synced_with_no_flush` (`:359`). Both default to `None`, so no other test changes.
- Each use shows that its hook acted. With the helper dropping what the hook returns, the test that uses it fails (plants X1 and X2, both killed, each by that test alone).
- They change the fixture's files, not the check: `tamper_marks` what goes into `marks.tsv`, `tamper_starts` the `status-start-K.txt` texts.
- One thing to know: `tamper_starts` is applied only where a holder is given (`:95` is inside `if holder is not None`). Without one it is ignored in silence, as `split`, `tamper_split` and `tamper_written` already are. Its one use passes a holder and asserts `instance_2` FAIL, so it cannot pass by being ignored.
- `test_a_failed_instance_line_is_a_verdict` asserts that `instance_2` is the only FAIL or ERROR line and that the exit is 1, so the exit can come from nowhere else. With the fault the other way (a failed instance line as the rig's error, O18) it fails too.

### 3. The listed plants, planted again here

All at 813120a, one at a time, from the older reviews' own scripts where their text still applies.

| Plant | From | Result here | Killed by |
|---|---|---|---|
| I7 | #111, first review | NOT APPLIED from the old script (its text predates the `begun` argument); planted in this head's text: KILLED | `test_a_failed_instance_line_is_a_verdict` |
| N5 | #111, first review | KILLED | `test_a_group_synced_with_no_flush` |
| H4 | #111, first review | KILLED | `test_the_program_ends_at_a_leave` |
| F4, F5, F6 | #111, second look | KILLED | `test_the_first_of_several_and_only_a_release` |
| L3, L4, L6, L7, L9, L10, L13 | #111, second look | KILLED | `test_instance_lines_and_the_commit` |
| I12 | #111, first review | SURVIVED, as the author says; I called it equivalent in that review | |

Each is killed by the new test alone: before 5ed17fe every one of them survived or, for I7, did not apply.

**The whole of #111's two lists at this head** (`plants-2b-813120a.out`, `plants-2b-next-813120a.out`): 82 plants, 61 killed, 1 survived (I12), 20 not applied. Of the 20, K2, K5, B3, B4 and I7 are killed in this head's text, and the text of the other 15 was gone before this pull request. Nothing of #111's lists is open any more except I12.

### 4. No new test is vacuous

For each new test I planted faults the other way round from its plant (`plants-look-813120a.out`, O1 to O18): 17 of 18 killed.

| New test | Faults the other way | Result |
|---|---|---|
| `test_the_first_of_several_and_only_a_release` | no release answered is ever a commit; the time asked and not answered; only a release on the log's first line counts; a malformed line is the rig's error | all four KILLED, the last by this test alone |
| `test_instance_lines_and_the_commit` | a later start takes either map whenever; never takes either; every later start is a verdict; no start is a verdict | all four KILLED |
| `test_the_program_ends_at_a_leave` | a leave ends with 1; writes no `left` line; the holder steps once before it leaves; writes its final listing at a leave | all four KILLED, three by this test alone |
| `test_a_group_synced_with_no_flush` | O14 to O17 above | all four KILLED |
| `test_a_failed_instance_line_is_a_verdict` | a failed instance line is the rig's error | KILLED |

The one survivor is O9: a commit at the very nanosecond of a start counts as before it or after it (`>` to `>=` at `cut_daemon_check.py:1113`). No test has a commit at a start's own time. It has no weight: the two clocks do not meet on a nanosecond.

A note on `test_the_program_ends_at_a_leave`: the leave file is there before the holder starts, so the holder leaves before its first step. The test holds "no drain, no final listing" (H4), and O12 shows it also holds "no step before the leave". It does not show a holder that has stepped and then leaves with a cut half written; `test_pause_then_leave` is the nearer test for that.

### 5. G1, the page by a number

**Closed.** `Parse.test_a_tear_on_a_page_of_4_kib` (`test_cut_daemon_check.py:946-960`) puts the next start at bytes 4096, 8192 and 12288 of the file, written as numbers, and asserts the position.

| `PAGE` | Result | Killed by |
|---|---|---|
| 8192 | KILLED | `test_a_tear_on_a_page_of_4_kib` |
| 16384 | KILLED | `test_a_tear_on_a_page_of_4_kib` |
| 12288 | KILLED | `test_a_tear_on_a_page_of_4_kib` |
| 65536 | KILLED | `test_a_tear_on_a_page_of_4_kib` |
| 2048, 1024, 512 | KILLED | `test_a_torn_line_not_followed_by_a_start` |
| 1 (a tear anywhere) | KILLED | `test_a_line_torn_anywhere_on_a_page`, `test_a_torn_line_not_followed_by_a_start` |

The two `PAGE` plants that survived at fe6970c are killed, and the 21 older plants I had re-planted in this head's text are now all killed (`plants-again-813120a.out`).

### 6. F1, the words of `obs_unstored_at_kill`

**Closed.** `cut_daemon_check.py:1035-1046`. The line now reads "N flushes killed after their barrier and before their answer; a cut such a flush carried was not stored", and a shape B's note "barrier with no cut line, taken as a flush with no barrier (whether or not it carried a cut)".

- That is true of every barrier at the end of an instance, with a cut or without. It no longer says the rig hit D1's window.
- No verdict changes: the 28 recordings of the run on this head, judged here with this head's check, end with exit 0 and the run's own lines (`rejudge-813120a.out`).
- Both tests that read the line ask for the new words, and the old words fail one of them (plant W1).
- The author did not make the reference say whether a mark was waiting at the barrier, and says so. That is fine: the words claim nothing they cannot show.

### 7. The description's corrections

All three are made.

| Asked | Now |
|---|---|
| The heads after afd217a named | `fe6970c`, `5ed17fe` and `813120a` are in the commit list, each with what it is for, and the CI section has a line for each head |
| "The review's figures counted copies another way" | gone; the description gives the seven recordings counted both ways and says the 20, 69 and 28 were other recordings |
| `cut_ref.py` named as changed | "`test_cut_ref.py`; `cut_ref.py` itself is unchanged" |

- F2, R1, R2, G2 to G5, S1 and S2 are listed for the next package, each in a line, as none blocking. I agree with that sorting.
- One line to correct with N1: the table's row for N5 says "two flushes gone inside a `noflush` span".
- "CI on `813120a`: to follow" can be filled in now (below).

## Where my earlier findings stand

| Finding at fe6970c | At 813120a |
|---|---|
| F1 wording | fixed |
| G1 the page by a number | fixed |
| F2, R1, R2 | open, named for the next package; no course reaches them |
| G2 (H8), G3 (H10, A6), G4 (U4, U10, U11, U14, U15), G5 (H7, L7, T5) | open, named for the next package; the same 14 plants of my list survive as at fe6970c, with H2, H3 and L8, which compute the same number either way (`plants-head2-813120a.out`: 59 plants, 45 killed, outcomes identical to fe6970c's) |
| S1, S2 | open, named for the next package |
| #113's survivors A7, K9, Z4, Z7, P7, P11, W4 | survive still; the description names A7 and K9, and P7 and P11 were the author's own |

## CI and Codex on 813120a

- `ci` 44226456740, `coverage` 44226456713, `e2e` 44226456730 and the push's `skyzfs-ci` 44226451385: green.
- The cut rig on the daemon 44226456809: green on both images, 14 recordings exit 0 each. `held_across_start`: 4 blocks on ubuntu-26.04, 281 on ubuntu-26.04-arm. `carried_across_failure`: 1780 and 1789 of 6177. `obs_unstored_at_kill`: 0 in all 28.
- The other rigs were cancelled by the author. 813120a changes `cut_daemon_check.py` and two of its test files, which only the cut rig on the daemon runs; I checked that at fe6970c by reading the workflows, and no workflow changed since.
- Codex's review of 813120a is complete with no findings.
- The local tests pass here at 813120a: `test_cut_ref` 41, `test_cut_check` 26, `test_cut_hold` 18, `test_cut_daemon_check` 93, `test_cut_daemon_main` 24, and the two shell tests.

## What I could only read

- Nothing that needs a kernel ran here. The recordings are CI's; I judged them again and read their spans.
- That a real pool sends the flush after `noflush-end` rests on the six recordings and on the recorder's script, not on a run of mine.
- shellcheck is not installed here; no shell file changes in these two commits.

## Evidence

All in `review/m2.t3-2d/` on `skyzfs-review`.

| File | What it is |
|---|---|
| `n5_model.py`, `n5_model.out` | the test's log and five other logs of the same span through the program |
| `n5_real.py`, `n5_real.out` | what six real `takeover` recordings hold between the marks |
| `plants.py` (set `look`), `plants-look-813120a.out` | I7 and N5 in this head's text, the page by eight numbers, the faults the other way round, the helper's hooks, the old words |
| `plants-2b-813120a.out`, `plants-2b-next-813120a.out` | #111's two lists, whole, at this head |
| `plants-head2-813120a.out`, `plants-again-813120a.out`, `plants-2c-survivors-813120a.out` | my own list, the re-planted older plants, and #113's survivors at this head |
| `rejudge.py`, `rejudge-813120a.out` | the 28 recordings of run 44226456809 judged here |
