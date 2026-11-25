# Third look at skyzfs/skyzfs#150 (M2.6c-4: the kill points `carry-put:N` and `carry-swap:N`)

| | |
|---|---|
| Head judged | `c429aab` (`c429aabf7260d6686fa401b15ab5a91b18862f1c`); unchanged when this note was written |
| Base | `a5e883a` (the tip of `zfs-topology`) |
| New since `d629834` | `aa29399`, the author's commit (`bins/skyzfsd/src/{kill.rs, kill/tests.rs}`). `c429aab`, a merge of `zfs-topology` at `a5e883a` |
| The merge | clean. `git show --remerge-diff c429aab` is empty. In `skyzfsd` it changes only doc links in `cli.rs` and `upload.rs` (#151). It does not touch `skyzfs-upload`, so the references to `driver.rs` and `uploader.rs` in `next_look.md` still hold |
| What was read | `git diff d629834 aa29399`, against `next_look.md` beside this note and the author's answer (issue comment 7176279810) |
| Codex | completed on `c429aab` with no findings |
| CI on the head | CI_HEADLINE |

## Verdict

**VERDICT_LINE**

- **R2 is fixed, for both failure modes.** `KillRemote::publish` tells the hook on any error from the inner call (`kill.rs:828-835`). That covers the write that landed with its answer lost (R2) and the write that failed before it landed (R1). The hook prints one give-up line and never kills afterwards.
- **The test pins R2.** `a_publish_n_whose_call_failed_gives_the_hook_up` (`kill/tests.rs:1221`) covers both kill points, landed and not, and asserts the hook's lines in full. It fails when the fix is reverted (plant V1). My two scratch tests that showed R1 and R2 now fail at this head, as they should.
- **T's docs are accurate on the three points I raised**: the `Turns::next` order, `carry-put`'s window ending at the first segment PUT, and what the rig's comparison sees.
- **One qualification for the lead (D1).** Under `carry-put`, the design itself allows a difference between the kill line's cut and the row's after the restart. The docs' "a difference is a finding against the daemon" does not allow for it.
- **G1 and G2 are closed.** Plants N5, N6 and N1 are killed by the pull request's own tests.
- **Plants: 32 of 33 killed at this head**, among them the seven new ones on R2's fix (V1 to V7). Only M16 survives, as before, and it cannot be reached deterministically.
- **Gates are clean at the head.**

## Findings, most severe first

None blocks the merge.

### D1. Decision for the lead, certain by the design's text, rare in a run: under `carry-put` a difference after the restart can be the design's, not a fault

The docs at `kill.rs:86-89` end: "A rig that compares the kill line's cut with the cut the snapshot's row names after the restart sees the same cut in this course; a difference is a finding against the daemon." This is the lead's ruling.

**Under `carry-swap` it holds.** The cut the kill line names has committed and was not let go. After the restart it is found committed, and the publish asked again goes on it.

**Under `carry-put` the named cut has not committed at the kill.** After the restart it can still be abandoned: at the bound, or where its read or its sync fails (`crates/skyzfs-upload/src/driver.rs:22-26`). The publish then lands on the cut that took its map. The design allows this explicitly: `crates/skyzfs-upload/src/publish.rs:13-16`, "across a restart, a publish can so land on a later cut than the first counted after *C*; that cut qualifies all the same" (the lead's Q2 of M2.5). The row then names a later cut than the kill line, with no fault in the daemon.

This applies to every `carry-put` course, not only to T's window.

**Suggestion.** Qualify the sentence: "…a difference is a finding against the daemon, unless, under `carry-put`, the restarted serve abandoned the cut the kill line names". The rig (testing's item 7) then needs a way to see that abandonment before it judges. I did not check what the daemon reports when a cut is abandoned at the bound.

### D2. Nit in the docs, from `d629834`: the general rule says a failed call is never killed at

`kill.rs:23-24`: "Only a PUT, a commit or a publish that came back as landed is counted: one that failed or was refused is not, and is not killed at."

Since F2, `carry-swap` does kill at a commit whose call failed but which landed: it is seen as the cut is let go (`kill.rs:50-53`). Suggest "…is not killed at, but for `carry-swap`'s chosen cut (below)". I missed this at `d629834`.

## What was asked

### R2's fix and its test

**The code** (`kill.rs:828-835`). When the inner publish returns an error, `KillRemote::publish` calls `Carry::publish_failed(name, cut)` and then returns the error. `publish_failed` (`kill.rs:389`) and `published` (`kill.rs:378`) share `gives_up_at` (`kill.rs:395`). Where the name is publish N's and the hook has not fired, it:

- sets `done`;
- prints `publish N (name) may have been made (its call failed) on cut X (group G), {the cut chosen | which the hook did not choose}, before the hook fired; no kill`.

**Landed and not landed.** The call is the only place a publish writes. The lookup is `Remote::snapshot`, and the manifests' PUTs and the refs transaction are all inside `Remote::publish`. So any error in the publish's writes reaches the hook. The hook cannot tell a write that landed (R2) from one that did not (R1), and gives up on both. The docs (`kill.rs:65-69`) and `publish_failed`'s own doc say this is the conservative reading.

**A step is not dropped half done.** While the uploader runs, its step is not cancelled: `step_keeping` in `uploader.rs` polls it to its end ("neither it nor a renewal is dropped half done"). So no path leaves the hook untold after a publish's write was sent.

**No kill after giving up.** Two guards stop it:

- `reads` chooses nothing once the hook is done (`kill.rs:441`);
- `fires` returns nothing once it is done (`kill.rs:474-476`).

The test asserts the lines in full: the arming line, then the give-up, and nothing else. That holds although cut 8 is checked, read and committed after the give-up, and the post follows the publish.

Each guard is needed, and each is pinned:

- **V5** removes the first guard. The R2 test and two others catch it: a cut read after the give-up is chosen again and says so.
- **V6** removes the second guard. It is caught by `a_publish_n_that_ends_without_being_made_gives_the_hook_up` and two of the firing tests, where the hook would fire at a cut chosen before it gave up, or fire a second time. In the R2 course itself no cut is chosen before the give-up, so the first guard holds there.

**The test** (`a_publish_n_whose_call_failed_gives_the_hook_up`, `kill/tests.rs:1221`):

- It is my R2 course, for `carry-swap` and `carry-put`, with the fault after the refs write (R2) and before it (R1).
- It asserts that the publish is made on cut 7 after the wait, and `found: true` only where the write landed.
- It then posts the ended publishes (G1) and asserts the lines in full.
- It fails when the fix is reverted (V1), when `publish_failed` does nothing (V2), when the give-up says nothing (V3), and when it does not set `done` (V4).

**My scratch tests at this head** (`run150t.out`):

- `r150n_show_r1_*` and `r150n_show_r2_*`, unchanged from the next look, now **fail**. Each expected a kill at cut 8 and found the hook giving up, with the new line. This is the fix seen from my side.
- `r150n_show_t_*` (cases A and B) and `r150n_carry_swap_at_the_release_fires_before_the_cut_is_let_go` pass, as the pull request's copies of them do.
- **New:** `r150t_a_failed_call_on_the_chosen_cut_gives_carry_put_up_naming_it` passes. It is Z with a failed call: `carry-put`, the chosen cut 7 puts no segment, and its publish's call fails after landing. The hook gives up with "…on cut 7 (group 7), the cut chosen, …", says nothing more through cut 8 and the post, and the publish is found on cut 7.

### T's docs (`kill.rs:71-89`)

1. **The order: accurate.** "the uploader gives the next step to the cut or to the publishes by the time each has had since both had work (`Turns::next` …), not a publish first as `Driver::step` does". That matches `uploader.rs:159-176` and `driver.rs:412-418`. "Where publish N's lookup took longer than the next cut's check and first read" is the plain case; the count runs from when both had work, which the sentence before says.
2. **`carry-put` ends at the first segment PUT: accurate.** "`carry-put` at its first segment PUT, `carry-swap` once its commit landed".
3. **What the comparison detects.** "sees the same cut in this course": accurate, and it is marked as reasoned from the code and not run. "a difference is a finding against the daemon" is the lead's ruling; with R2 fixed it holds for `carry-swap`, and D1 qualifies it for `carry-put`.
4. **The R2 sentence** (`kill.rs:65-69`) is accurate, including that R1 gives up too.

The pull request's `the_uploaders_turns_can_have_the_hook_kill_at_a_cut_after_the_carrier` (`kill/tests.rs:1304`) is my two T courses, for both hooks and both cases. It pins the window as the docs describe it.

### G1 and G2

- **G1.** N5 (`ended` without its `done` check) and N6 (`gives_up_at` without its `done` check) are now killed by `a_publish_n_whose_call_failed_gives_the_hook_up`, through the post it calls.
- **G2.** N1 (`carry-swap` told after the inner release) is killed by `carry_swap_at_the_release_fires_before_the_cut_is_let_go` (`kill/tests.rs:1389`), my test taken.

### Plants at `c429aab`

**How the plants were run.** Each plant was applied alone to a clean worktree of the head. Then `cargo test -p skyzfsd --features test-hooks -- kill:: carry the_test_hooks_are_parsed_in_a_test_build` ran with a 600 s limit, and the file was restored in a `finally` (`plants150t.py`). These are the plants of `plants150n.py` again, Q4 and Q6 run as Q4b and Q6b, plus the new V plants on R2's fix.

**32 of 33 KILLED; only M16 survives, as before.** No plant hit the 600 s limit (`plants150t.out`).

| Plant | What it breaks | Result | Killed by (one of) |
|---|---|---|---|
| Q1 | `KillRemote::publish` does not tell the hook of a publish made | KILLED | the F1 course tests |
| Q2 | the ended publishes not read | KILLED | `a_publish_n_that_ends_without_being_made_...` |
| Q3 | no fire at the chosen cut's release | KILLED | `carry_swap_at_the_release_fires_before_...`, `carry_swap_fires_as_the_chosen_cut_is_let_go_...` |
| Q4b | every acceptance counted | KILLED | `a_name_accepted_again_is_not_counted_again` |
| Q5 | any publish made, or failed, gives the hook up | KILLED | `a_name_accepted_again_is_not_counted_again` |
| Q6b | any publish ended gives the hook up | KILLED | `a_publish_n_that_ends_without_being_made_...` |
| Q7 | the chosen cut not told apart in the give-up line | KILLED | `a_publish_made_on_the_chosen_cut_with_no_segment_...` |
| Q8 | the release fires under `carry-put` too | KILLED | the same |
| Q9 | the release of any cut fires `carry-swap` | KILLED | `carry_swap_falls_after_...` |
| P9 | an ask whose asker went still waited on (B1's test) | KILLED | B1's test |
| M3, M4, M6, M7 | the bounds C and G; a later read as a new reading | KILLED | `a_cut_numbered_c_plus_1_...`, `a_cut_under_way_...` |
| M13 | `maker` does not give the hook to the objects | KILLED | `the_carry_hooks_fire_at_the_carrier_...`, `the_uploaders_turns_can_have_...` |
| M16 | an ask still waited on is kept after its asker went | SURVIVED | unreachable deterministically (`review.md`) |
| N1 | `carry-swap` fires after the release (G2) | KILLED | `carry_swap_at_the_release_fires_before_the_cut_is_let_go` |
| N2, N3, N4 | the counting of names | KILLED | the counting tests |
| N5 | `ended` without its `done` check (G1) | KILLED | `a_publish_n_whose_call_failed_gives_the_hook_up` |
| N6 | `gives_up_at` without its `done` check (G1) | KILLED | the same |
| N7 | a publish made on the chosen cut does not give up | KILLED | `a_publish_made_on_the_chosen_cut_with_no_segment_...` |
| N8, N9, N10 | the S1 lines | KILLED | the lines asserted in full |
| V1 | R2's fix reverted | KILLED | `a_publish_n_whose_call_failed_gives_the_hook_up` |
| V2 | `publish_failed` does nothing | KILLED | the same |
| V3 | the give-up sets `done` but says nothing | KILLED | the F1 tests and the R2 test |
| V4 | the give-up says so but does not set `done` (fires later) | KILLED | the F1 tests and the R2 test |
| V5 | `reads` chooses after the give-up | KILLED | the R2 test and two more |
| V6 | `fires` without its `done` check | KILLED | `a_publish_n_that_ends_...` and two more |
| V7 | a failed call reported as "was made on" | KILLED | the R2 test |

## Gates

They were run at the head on toolchain 1.100.0 (`rustup update stable` reported it current), on `skyzfsd`, with an own target directory and `CARGO_PROFILE_DEV_DEBUG=0`. The logs are beside this note (`third-gate-*.log`).

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy -p skyzfsd --all-targets -- -D warnings` | clean |
| the same with `--features test-hooks` | clean |
| `cargo test -p skyzfsd` | 142 passed, plus 3 in `closed_output` |
| `cargo test -p skyzfsd --features test-hooks` | 197 passed (194 at `d629834`, plus the three new tests), plus 3 in `closed_output` |
| my scratch tests (`run150t.sh`) | 4 pass; the 2 that showed R1 and R2 fail, as they should at this head (`run150t.out`) |

## CI on `c429aab`, read only

CI_TABLE

**Not covered.** No job starts a daemon with `carry-put` or `carry-swap` (M15, testing's item 7).

## Decisions for the lead

1. **D1.** Qualify "a difference is a finding against the daemon" for `carry-put`: a cut abandoned after the restart moves the publish to a later cut, as the design allows. Both the docs and the rig that drives `carry-*` should know this. A docs line; it does not block.
2. **D2.** A one-line docs nit. It can go with D1, or later.
3. **The merge.** MERGE_LINE

## Files beside this note

- `scratch150t.rs`: the scratch tests of this look, appended to `kill/tests.rs` for a run. They are the `r150n_*` tests unchanged, without the helpers the pull request took, plus `r150t_*`.
- `run150t.sh`: runs them against a clean worktree of the head and restores the file.
- `plants150t.py`: the plants, each alone, with a 600 s limit and a restore in `finally`.
- `run150t.out`, `plants150t.out`, `third-gate-*.log`: the outputs. Paths are replaced by `<worktree>`, `<target>` and `<scratch>`.
