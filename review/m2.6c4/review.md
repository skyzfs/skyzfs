# Review of skyzfs/skyzfs#150 (M2.6c-4: the kill points `carry-put:N` and `carry-swap:N`)

| | |
|---|---|
| Head judged | `aeeb3cd` (`aeeb3cdb60adf000f7243218e4a4a5732151edba`), unchanged when this note was written |
| Base | `8f0390f` (the tip of `zfs-topology`); commits `8f0390f..aeeb3cd` |
| Files | `bins/skyzfsd/src/{kill.rs, kill/tests.rs, upload.rs, upload/tests.rs, cli.rs, cli/tests.rs}` |
| Codex | completed on `aeeb3cd` with no findings |
| CI on the head | all green (list at the end) |

## Verdict

**Not ready to merge as it stands.**

- **B1 blocks.** One of the pull request's tests fails at random, about once in 1500 runs. The fix is one line in the test.
- **F1 needs a decision from the lead before the merge.** It is the pull request's open point 2. The hook can kill at a cut that does not carry publish N, and its kill line then says that it does. A rig judging that course passes without testing what it was meant to test. I recommend fixing it in this pull request. The fix is small: detect the case, say so, and do not kill. With it, change the wording "which carries".
- **F2 should be fixed together with F1.** `carry-swap` misses a carrier whose commit landed but whose call came back failed. It then kills at a later cut. This contradicts the module's own documentation. It is unlikely to happen but certain in the code.
- Everything else is a suggestion or a test gap.
- **The default build is unchanged.** I have no finding against the release daemon or against the other kill points.

## Findings, most severe first

### B1. Blocking, certain: `the_carry_hook_passes_the_channel_on_and_counts_the_publishes_accepted` fails at random

`bins/skyzfsd/src/upload/tests.rs:1709` and `:1718`.

The test means to check that an ask whose asker has gone is not taken. It sends the ask "w" with `blocking_send` and only then drops the receiver of the answer: `drop(send(ask("w", ...)))`. The hook's forwarder (`kill.rs:365-376`) runs on the uploader's thread. It can pull "w" from the inbox and pass it on before the test thread drops the receiver. At that moment the asker's reply is still open, so the hook queues the ask and gives the uploader an open answer channel (`kill.rs:232`). The uploader then takes "w", and the assertion `progress("w") == Some(Unknown)` fails:

```
panicked at bins/skyzfsd/src/upload/tests.rs:1718:5:
assertion `left == right` failed: not taken
  left: Some(Pending)
 right: Some(Unknown)
```

- **Measured.** I ran the test from its binary, one test per process. It failed 2 times in 2000 runs, 2 in 4000 and 2 in 3000: 6 in 9000. The output is in `flake-1.out` and `flake-2.out`, and the 3000-run count is at the end of `run150-first.out`.
- **CI exposure.** The test runs in `ci.yml`'s `test` and `unit-arm64` jobs on every push to a pull request, and in `skyzfs-ci.yml` after each merge.
- **The fix.** Drop the receiver before the send, which is what the test means. My copy, `scratch150_upload.rs`, changes only that and failed 0 times in 3000 runs (`run150-first.out`).
- **Not a product defect in itself.** The same window exists in the daemon, and the forwarder notes below describe it.
- **Why it blocks.** It is a test in the merge gate that fails without the behaviour breaking, and the fix costs one line.

### F1. Needs a decision (open point 2), certain in the code: the hook can kill at a cut that does not carry publish N, and its kill line says that it does

The hook chooses the first cut whose reading begins after publish N's acceptance (`Carry::reads`, `kill.rs:282-298`). The uploader does not choose that way. `skyzfs_upload::publish` gives a publish to the first cut that carries it, among the cuts the uploader already knows to be committed (`absent`, `publish.rs:832`; up to `KNOWN_CUTS` = 64, `publish.rs:133`) and the cuts committed after the publish. In two cases the carrier is a cut the hook will never choose:

- **A. The carrier's reading began before the acceptance.** The uploader answers the channel between its steps, so a publish can be accepted while the carrier uploads. Since the uploader polls every 250 ms when idle (`uploader.rs:133`), this happens whenever the ask arrives while the carrier is being read.
- **B. The carrier was committed before the acceptance.** The lookup gives the publish to the carrier at once, and that cut is never read again.

In both cases the publish is made on the carrier, cut 7. The hook then chooses the next cut that fits the predicate (state, number > C, group ≥ G, opens as itself) and kills there, at cut 8. The kill line names cut 8:

```
kill carry-swap:1: after the commit of cut 8 (group 8), which carries publish 1, landed, before the cut was let go
kill carry-put:1: after the first segment PUT of cut 8 (group 8), chosen to carry publish 1, landed (...)
```

- **How this was shown.** I drove the real `skyzfs_upload::Driver` through the daemon's own `kill::maker` (the test-hooks build's remote: `KillStore`, the engine, `KillRemote`), over a store in memory and a local store that holds whole cuts:
  - `r150_show_a_carry_swap_...` and `r150_show_a_carry_put_...` show case A, once for each kill point.
  - `r150_show_b_...` shows case B.
  - In each of them the step `Published { name: "t", number: 7, ... }` comes before the kill at cut 8 (`run150.out`).
- **The control.** `r150_control_carry_swap_...` asks while no cut is under way. There the kill falls after cut 7's commit and before the publish's transaction, as intended.
- **Why it matters.** The rig's verdict after the restart is "snap/t is absent and a retry publishes it, or it names a cut that holds it". That verdict holds whether or not the kill fell between the acceptance and the ref. So a course that hits A or B passes without testing M2.6's case. The line "which carries publish N" gives the rig no way to tell.
- **How likely it is.**
  - On the publish rig's `passist` course of this pull request's CI, the ask names a G that the pool has not reached yet: "a1 asked with G 20: the newest group that wrote was 16 when it was accepted". The carrier came after the acceptance, so the hook's choice would have been right.
  - A course that snapshots first and then asks, like `pplain`'s t1 and t2 (G is the snapshot's own group), races the upload of the carrier. There, B is likely whenever cuts are made and uploaded faster than the ask arrives, and A is likely under load.
  - I did not measure that race on a kernel.
- **Two related paths, not tested by me.**
  - Publish N can end without being made after its acceptance: dropped at its deadline, or refused after a rewind. The hook stays armed on its predicate and can kill at a later cut, saying the same.
- **The fix I suggest is small, and the author chooses its form.**
  - Keep publish N's name in `Wanted`.
  - Check in `KillRemote::publish` (`kill.rs:630-644`), which already sees `snapshot.name` and `snapshot.row.cut`. If publish N lands on a cut other than the chosen one before the hook has fired, print a line ("carry-swap:N: publish N was made on cut X (group Y), which the hook did not choose; no kill") and disarm.
  - For the dropped publish, the post's ended publishes could be passed to `Around::posted`, so that the hook disarms when publish N ends without being done.
  - Change "which carries publish N" in the carry-swap line to "chosen to carry publish N", as the carry-put line already says. The cut carries the publish only once its publish is made.
  - The rig should then require the kill line, and compare the cut it names with the row's cut after the restart.
- **Other options for the lead.** They are listed under "Decisions for the lead".

### F2. A defect against the module's own documentation, certain, unlikely: `carry-swap` misses a carrier whose commit landed but whose call failed

`kill.rs:612`: `let commit = self.inner.commit(cut, expected).await?;` returns on the error before `carry.committed(cut)` (`kill.rs:620-622`).

- **The driver's path.** When a commit's call fails, the driver observes the refs row. If the row names the cut, the commit had landed, and the driver lets the cut go through `uploaded` (`driver.rs:722-730`, then `driver.rs:772`). `Remote::commit` never answers Ok for that cut. So the hook never sees the chosen cut's commit. It chooses the next cut whose reading begins and kills after that cut's commit, with "which carries publish 1". Meanwhile publish 1 was made on the first cut.
- **How this was shown.** `r150_show_d_...` arms a fault on the refs object's write that lands first and then reports an error. The steps run:
  1. `Err(Store("refs: ... injected fault (after the operation completed)"))`
  2. `Uploaded { number: 7 }`
  3. `Published { name: "t", number: 7 }`
  4. cut 8 is read and committed, and the kill comes there:

  ```
  kill carry-swap:1: after the commit of cut 8 (group 8), which carries publish 1, landed, before the cut was let go
  ```
- **What the documentation says.** The module documentation says `carry-swap:N` "fires once the chosen cut's commit has landed" (`kill.rs:20-21`, `:48`), and that the next cut is chosen only where the chosen cut ends without the hook firing (`kill.rs:41-43`). Here the chosen cut's commit did land.
- **How likely.** Low. It takes a refs write that takes effect but whose answer is lost. The fix of F1 covers it: publish 1 lands on cut 7, which the hook still holds as chosen, and the hook disarms with a line.
- **A more exact fix.** `KillLocal::release(number)` for the chosen cut is called before the cut is let go. If the hook has not fired by then, it can fire under `carry-swap`, at the very point that kill point names, after the commit landed and before the cut is let go.

### Z. Low, documented: `carry-put` passes over a carrier that puts no segment

`kill.rs:52-53` documents this: "A chosen cut that puts no segment ... is passed over at its end, and the next is chosen".

- **What happens.** That cut still commits and carries the publish. The hook then kills at the next cut's first segment, with "chosen to carry publish N". `r150_show_z_...` shows it: publish 1 is made on cut 7, whose blocks are all zero, and the kill comes at cut 8.
- **How likely.** In practice this is very unlikely, because a ZFS group that holds a snapshot always writes metadata.
- **The fix.** The fix of F1 turns this case into a line and no kill.

### S1. Suggestion: an unreached hook says nothing

When publish N is never accepted, or no cut is ever chosen, the daemon prints nothing about the hook (`r150_show_n_...`). A rig whose course went wrong (F1, open point 3, a state that changed) sees a serve that ran to its end with no line to explain why.

I suggest one line on standard error when the hook arms (publish N accepted, with its name, C and G), when it chooses a cut, and when it gives a choice up. The rig should treat a missing kill line as its own error (exit 2), not as a verdict.

### S2. Suggestion, open point 3: count distinct names, or say N = 1 per serve

`answered` counts every `Accepted` (`kill.rs:255-259`). The uploader answers `accepted` again to a publish asked again while it pends. The publish rig of this pull request's CI shows it: `pplain: pin_answer_t1-again: ok: [server] answered 'accepted'`. The agent asks again after `no-answer`, `ended`, `unknown` and `refused rewound` (`crates/skyzfs-agent`, `plan.rs`).

So with N ≥ 2, publish N can be a second ask of publish 1. `r150_show_3_...` arms `carry-put:2` on two asks of the same name "a", and the hook kills with "chosen to carry publish 2".

- **For N = 1** this is harmless.
- **The cheap fix.** Count the first acceptance of each name in the serve. Otherwise, document that rigs use `carry-*:1` and ask once per serve.

### Notes on the forwarder: minor, test-hooks build with `carry-*` only, not blocking

`Carry`'s forwarder (`kill.rs:365-376`) puts one more channel (capacity 1) and one task between the channel handler and the uploader. Against the plain build:

1. **Up to two more asks in flight.** That is one in the hand-off channel and one held by the forwarder, beyond `INBOX` = 16. An ask is therefore refused as busy a little later.
2. **The asker's absence is checked earlier.** It is checked when the forwarder passes the ask on (`kill.rs:232`), not when the uploader reads it. An ask whose asker leaves while it waits in the hand-off is still taken. Such an asker is, for example, the handler at its 30 s limit, while the uploader is in a long step. The plain uploader would have skipped that ask. The hook counts it, and that count is true to what the uploader accepted. This is B1's window, and it is short unless a step is long.
3. **The inbox lives on after the run ends.** Once `run_with` returns, the forwarder keeps the inbox until the next ask arrives or the runtime is dropped. A late ask is then dropped, rather than refused by `try_send` at once. The asker reads `ended` or `no-answer` later than in the plain build.
4. **Order and panics.** The order of requests is kept. `Carry::seen` recovers from a poisoned lock. Nothing in `ask` can panic. If the forwarder ends, the uploader's inbox closes, as it does at the end of a serve.

None of this reaches the default build, or a test-hooks build run with any other kill point.

## The pull request's open points

1. **"carry-put:N cannot know at the PUT whether the cut will commit."** This is not a real gap, and it does not block. `die` in the daemon is `kill::sigkill`, which always ends the process. The case "if the daemon is not killed there" exists only in unit tests, whose `die` records. The conservative reading is right.
2. **"A cut whose reading began before publish N was accepted is never chosen."** This is a real gap, and it is F1 above. The pull request's advice, "ask the publish while no cut is uploading", covers case A only. It does not cover case B, where the carrier was committed before the ask. It needs the lead's decision, and at the least the rig must check the cut the kill names against the row.
3. **"N counts acceptances, not distinct names."** This is real, because asks are repeated both on the rig (`t1-again`) and by the agent. It is harmless for N = 1. See S2. It does not block, and it is a decision.
4. **Kernel coverage.** Agreed. Nothing in CI starts a daemon with `carry-*`. See the test gaps.

## The choice of cut against design §5.2

`Wanted::chooses` (`kill.rs:138-140`) is `Request::carried_by` without "opens as itself": the same state, number > C, group ≥ G. The "opens as itself" check is made when the hook fires (`kill.rs:303`). Against the definition of a cut and of the carrier:

- **The bounds are right.** C is exclusive and G is inclusive. A cut numbered C, or numbered at most C with a group of G or more (a rewind's), is not chosen. Nothing in the pull request's tests sits at either bound: plants M3, M4 and M6 survive them. `r150_bounds_...` pins all three and kills them.
- **Only a reading from block 0 counts as a new reading.** A read of the cut under way, from a later block after the acceptance, does not make it chosen (`kill.rs:283-285`). The pull request's carry-put test reads from block 512 only in cut 9, which is already chosen (`kill/tests.rs:568`). A new reading there would change nothing, so plant M7 (every read counts as a new reading) survives it. `r150_a_cut_under_way_...` pins the rule as built and kills M7.
- **The wrong-cut cases I found** are F1 (A, B, and a publish that ends without being made), F2, and Z. All of them rest on the same difference: the hook predicts the carrier, but only the uploader decides it.
- **Not tested by me:** a rewind between the acceptance and the carrier (rule R). The state changes, so `chooses` fails and the hook stays silent: S1.

## The `Around` wiring

- **The default build.** `upload.rs:806-811` passes `Plain` (`upload.rs:837-854`):
  - `local` returns the `Queued` it was given, with the same type as before;
  - `inbox` returns the same receiver and spawns nothing;
  - `posted` does nothing.

  Once monomorphised, `Uploading::start` (`upload.rs:1118-1131`) does what it did before. `Around` and `Plain` are compiled into the default build but cost nothing. The only signature change is a new generic parameter on the public `Uploading::start`, whose only callers are `Uploads::begin` and one test (`upload/tests.rs:1259`).
- **Where the count is taken.** `around.inbox` is called inside `runtime.block_on` (`upload.rs:1158`), so the forwarder's `tokio::spawn` has the uploader's current-thread runtime. `around.posted()` is the first call of the post closure (`upload.rs:1161`). The uploader posts right after each request it answers, before its next step, so an acceptance is counted before the uploader reads anything else. That is the position the choice needs.
- **Other kill points.** `Carry::new` (`kill.rs:197-209`) returns `None` for every other kill point. Then `Uploads::begin` passes `Plain` (`upload.rs:803`), and `maker` gives `KillStore` and `KillRemote` no carry, so their `if let Some(carry)` does nothing. `--kill-at-upload` takes one value, so `carry-*` cannot be combined with `put:N`, `swap:N`, `manifest:N` or `publish:N`. Those four behave as before. The publish rig's `pkill`, `pkillman` and `pkillpub` courses passed on this head's CI.
- **No release invariant is weakened.** The parser takes the hook flags only in a test-hooks build (`cli.rs:739`, `parse_with(args, cfg!(feature = "test-hooks"))`). Nothing that comes over the channel can arm a hook.

## Do the tests fail when the behaviour breaks?

I applied each plant alone to a clean worktree of the head, ran `cargo test -p skyzfsd --features test-hooks -- carry the_test_hooks_are_parsed_in_a_test_build` with a 600 s limit, and restored the file whatever happened (`plants150.py`). A second pass ran the plants that survived again, with my scratch tests appended (`SCRATCH=scratch150.rs`).

| Plant | What it breaks | The pull request's tests | With the scratch tests |
|---|---|---|---|
| P1 | refusals counted too | KILLED | |
| P3 | the cut under way at the acceptance chosen | KILLED | |
| P4 | "opens as itself" not checked | KILLED | |
| P5 | no new choice once a cut is chosen | KILLED | |
| P6 | the commit of another cut fires carry-swap | KILLED | |
| P8 | an answer not passed on to the asker | KILLED | |
| P9 | an ask whose asker went still waited on | KILLED | |
| P10 | a segment PUT fires under carry-swap | KILLED | |
| P11 | any PUT fires carry-put | KILLED | |
| M1 | publish N armed one acceptance early | KILLED | |
| M2 | publish N armed one acceptance late | KILLED | |
| M3 | a cut numbered C chosen | SURVIVED | KILLED |
| M4 | a cut of group G not chosen | SURVIVED | KILLED |
| M5 | G not checked | KILLED | |
| M6 | C not checked | SURVIVED | KILLED |
| M7 | a read from a later block counts as a new reading | SURVIVED | KILLED |
| M8 | fires more than once | KILLED | |
| M9 | carry-put does not compare the reading with the chosen cut | SURVIVED | SURVIVED (equivalent, below) |
| M10 | the post does not read the answers | KILLED | |
| M11 | carry-swap fires before the commit landed | KILLED | |
| M12 | carry-put fires before the PUT landed | KILLED | |
| M13 | `maker` does not give the hook to the objects (carry-put never fires in the daemon) | SURVIVED | KILLED |
| M14 | `maker` does not give the hook to the remote | KILLED | |
| M15 | `Uploads::begin` puts `Plain` around the uploader even with a carry hook | SURVIVED | SURVIVED (no unit test can reach it) |
| M16 | an ask still waited on is kept after its asker went | SURVIVED | SURVIVED |
| M17 | carry-put and carry-swap swapped in `Carry::new` | KILLED | |
| M18 | the parser takes carry-put for carry-swap | KILLED | |
| M19 | the offer of no cut forgets the cut offered | SURVIVED | SURVIVED (redundant, below) |
| M20 | the cut read is not checked to be the one offered | SURVIVED | SURVIVED (redundant, below) |

- **The author's plants.** P1 to P11 are nine of the author's eleven, all caught. I did not plant P2 or P7 again, because each needs a rewrite of several lines.
- **M9 is equivalent.** `put_landed` fires only for the chosen cut. The chosen cut is set at its reading, and replaced at the next reading from block 0, so the reading is always the chosen cut when a segment PUT lands.
- **M19 and M20 are redundant.** The uploader reads only the cut it was last offered.
- **M16's window.** An asker can go between the forward and the answer. That window is microseconds wide, and a unit test cannot reach it deterministically.

## Test gaps

- **The bounds** C and G, and a cut at most C with a group of G or more (M3, M4, M6). `r150_bounds_a_cut_numbered_c_plus_1_of_group_g_fires` could be taken as it is.
- **A continued read of the cut under way** (M7). `r150_a_cut_under_way_read_on_after_the_acceptance_is_not_chosen` could be taken as it is.
- **`maker` giving the hook to the objects** (M13). The carry-put test builds its `KillStore` by hand. The `r150_show_*` tests drive the real `Driver` through `kill::maker` and catch this. One of them, written as a positive test, would close the gap.
- **The wiring in `Uploads::begin`** (M15). No test covers it, and only a rig that starts `skyzfsd serve --kill-at-upload carry-*` can. No workflow in this pull request does, so this is testing's item 7. Until a rig runs `carry-*`, CI has not shown that the hook works in the daemon.
- **F1 and F2** have no test. The `r150_show_*` tests can become tests of the fix: no kill, and a line saying why.
- **B1's test**, once fixed.

## Gates

They were run at the head on toolchain 1.100.0, which was current stable when checked, on `skyzfsd`, the only crate changed. I did not run the workspace-wide tests. The logs are beside this note.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean (`gate-fmt.log` is empty) |
| `cargo clippy -p skyzfsd --all-targets -- -D warnings` | clean |
| the same with `--features test-hooks` | clean |
| `cargo test -p skyzfsd` | 142 passed, plus 3 in `closed_output` |
| `cargo test -p skyzfsd --features test-hooks` | 185 passed, plus 3 in `closed_output` |
| `cargo doc` on `skyzfsd`, plain and with test hooks, at the base and at the head | the same errors at both, with only line numbers moved; the pull request adds none |
| my scratch tests (`run150.sh`) | 10 of 10 pass at the head (`run150.out`) |

`run150-first.out` is the first run of the scratch tests. Its five failures came from a matcher in my scratch file (`"Published {"` against `Ok(Published {`), which I fixed before `run150.out`. It is kept for the repeat counts at its end.

## CI on `aeeb3cd`, read only

All of these were green:

| Run | Workflow | What it covers |
|---|---|---|
| 45676659692 | skyzfs-ci | the branch push |
| 45676832203 | ci | `test` and `unit-arm64` |
| 45676832133 | coverage | |
| 45676832204 | e2e | |
| 45676832125 | publish rig | `pplain`, `prewind`, `pkill`, `pkillman`, `pkillpub`, `passist` |
| 45676832244 | daemon rig | 24.04, 26.04 and 26.04-arm, on the daemon |
| 45676832271 | cut-daemon | |
| 45676832193 | replay rig | |

What CI does not cover:

- No job starts a daemon with `carry-put` or `carry-swap` (M15, F1, F2).
- B1 can turn `test` or `unit-arm64` red at random.

## Decisions for the lead

1. **F1, which reading.**
   - **(a)** Keep "the first cut whose reading begins after the acceptance". Add the check on the publish described above: say so and do not kill when publish N lands on a cut the hook did not choose, or ends without being made. Change "which carries" to "chosen to carry". The rig requires the kill line and compares its cut with the row's. **This is my recommendation, in this pull request.**
   - **(b)** Also choose the cut under way at the acceptance (the author's plant P3). This handles case A but not case B, and it changes your wording.
   - **(c)** Leave the hook as it is, and make the rig compare the kill line's cut with the row's cut after the restart, ending with exit 2 when they differ. This costs no code here, but every rig that uses the hook must know to do it.
2. **F2.** Fix it with F1 (the check on the publish covers it). Or fire `carry-swap` at `KillLocal::release` of the chosen cut when no commit was seen.
3. **Open point 3.** Count distinct names, or require N = 1 per serve.
4. **Whether B1 blocks.** In my view it does: a flaky test in the merge gate, and a one-line fix.

## Files beside this note

- `scratch150.rs`, `scratch150_upload.rs`: the scratch tests. They are appended to `kill/tests.rs` and `upload/tests.rs` for a run.
- `run150.sh`: runs them against a clean worktree of the head (`WORKTREE`, `CARGO_TARGET_DIR`, `REPEAT`) and restores the files.
- `plants150.py`: the plants, each alone, with a 600 s limit and a restore.
- `run150.out`, `run150-first.out`, `flake-1.out`, `flake-2.out`, `plants150.out`, `plants150-scratch.out`, `gate-*.log`: the outputs. Paths are replaced by `<worktree>`, `<base-worktree>`, `<target>` and `<scratch>`.
