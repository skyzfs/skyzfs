Review of #119 at 32d8d67 (M2.T3 stage 3: the cut rig on the daemon's uploads)

**Verdict: no defect that blocks the merge. One condition before it merges, which is the author's own open item: the upload stage has not run on a kernel at this head.** The last rig commit with an upload run is 6f26fca (`unostore` only), and every run was on #118's e42ea6a. `upload_balance` (32d8d67) has never read a real status that has `cuts_replaced`, and the `lease_holder` want has passed only in `ulost`, where the holder is node B's. I would merge after one dispatch of courses (a) to (d) at this head with `daemon_ref` set to #118's current head (8fae8f7 when I looked), with its table read. Whether to merge the rig before #118 settles is the lead's call.

I found four defects in the rig, none blocking. Three can turn something that is not the daemon's fault into exit 1, a verdict against it (D1, D2, D3). The fourth is that `ufailed` shows nothing of what it claims about the first cut after a failure (D4). There are also gaps and suggestions below.

Method: I read the diff `origin/zfs-topology...32d8d67` and #118 at 8fae8f7 (the rules in `crates/skyzfs-upload/src/check.rs`, the driver, `status.rs`, `serve.rs`) before the description. Then:

- 129 textual plants, one at a time, against the head's tests (`plants.py`, `plants.out`).
- Six scenarios (`scenarios.py`, `scenarios.out`).
- The head's judge on the recordings of the four kernel runs (`rejudge.py`, `rejudge.out`).
- Two measurements on those recordings: what node B's fetches could tell apart (`fetch_power.py`, `.out`) and what `ufailed`'s content line could (`ufailed_map.py`, `.out`).

Nothing here ran on a kernel. The head's tests pass locally: `test_cut_watch.py` 5, `test_cut_daemon_main.py` 35, `test_cut_daemon_check.py` 99, `test_cut_hold.py` 18, `test_cut_record.sh`, and `cargo test -p skyzfs-nodeb` 11. CI on the head: the no-kernel jobs are green and stages 1 and 2 through `skyzfsd` are green on ubuntu-26.04 and ubuntu-26.04-arm; other kernel jobs were still running when I looked. Codex has one finding (P2, cut_watch.py:140), which is D2 below; I reached it independently and S5 reproduces it.

## The three questions

### 1. Can node B's watcher and `upload_lines` pass a daemon that uploaded a wrong cut?

**Not for a cut node B fetched, as far as I can find. Yes for a cut it did not fetch, and it fetched one to three cuts per recording.**

The pairing of an image with a row holds. `step` (cut_watch.py:152-179) reads head and the row, fetches `head`, reads both again, and keeps the fetch only when both are unchanged. A commit's id does not come back, so an unchanged head across the read means the image is that commit's. `row` reads head and then the row in two calls, so a pair can be torn, but a torn pair before the read never equals the pair after it, and the fetch is dropped. I found no order of a daemon's commit, node B's `move-head` and `set-row` that keeps an image under a row it does not belong to. The judge then holds the row's number and group to the cut line and every block to the reference at that cut's instant, follows the image through the fetches of another writer's head, and fails a number at head under two commits.

Plants on this path are killed: the after-check removed (W01), the same cut fetched again (W03), zeroed blocks (W07, W14, W15), the diff's bookkeeping (W08, W09, W23), the columns (W21), another writer's head or row taken as the daemon's (J01, J02), a number twice (J05, J05b), a later state's numbers (J06), the blocks since the last fetch (J09), the last cut (J13, J13b, J14).

Three plants there survive, so the tests do not hold these properties, though the code has them:

- W02a and W02b: the after-check with head alone, or the row alone. `test_head_moved_under_the_read` moves both together. In the courses they move apart: `move-head` moves head and not the row, `set-row` the row and not head.
- J04: the image not followed through a skipped fetch. `test_the_daemons_fetches` gives the same result either way, because the daemon's next fetch in the test puts back both blocks the other writer changed. With J04 planted, a daemon that uploaded a delta onto the other writer's head and kept its block would be judged against an image that never had it.

What node B did not fetch is not judged, and there is no floor. S2: a recording of which node B fetched the last cut only passes (`1 of 26 cuts`). In the kernel runs (`fetch_power.out`):

| Recording (run) | Cuts counted | `uploaded` (last instance) | Fetched by node B | Blocks that differ between the cut's instant and the volume when node B read |
|---|---|---|---|---|
| life (44253158409) | 26 | 2 | 1, 26 | 12068; 4 |
| ukill (44253158409) | 466 | 9 | 1, 185, 466 | 50403; 26121; 4 |
| ukillput (44255421061) | 227 | 3 | 1, 123, 227 | 39730; 27540; 4 |
| urow (44253158409) | 22 | 8 | 18, 22 | 0; 4 |
| ulease (44253158409) | 18 | 2 | 18 | 4 |
| ulost (44253158409) | 18 | 2 | 10 | 4 |
| umoved (44255421061) | 15 | 2 | 8 | 0 |
| ufailed (44255421061) | 18 | 2 | 10, 18 | 4; 4 |
| unostore (44259950077) | 12 | 2 | 12 | 4 |

The last column is the stage's power against a daemon that uploads the volume as it is when it reads, and not as it was at the cut's instant. It is large for the first whole cut under load and for one cut in the middle of `ukill` and `ukillput`. Everywhere else it is the two writes after the last flush (4 blocks), or nothing. A delta cut (rule 2) was seen by node B under load twice in all the runs (185 and 123). That is what a watcher that looks twice a second gets from a daemon whose first upload takes three seconds; it is a limit to state, not a fault of the judge. Suggestion G1 below.

### 2. The course wants in `course_wants`

**`umoved` and `ulost` are held by what node B reads, and I found no wrong daemon that passes them. `ufailed`'s want follows the lead's reading, but the course cannot tell a right first cut from a wrong one (D4) and can fail a right daemon (D1).**

- **umoved** (cut_daemon_check.py:1680-1690). Independent of the daemon: head at the end is the other writer's commit, the row is the one read before the move, and the last cut fetched is that row's (`STOPPED_AT`). The daemon's own word: at least one mismatch, at least one cut abandoned, `head_moved`, `uploaded` as before the move. A daemon that uploads after the move changes head, and the three independent checks fail (`test_a_course_that_stops_the_uploads`). Plants J20, J20b, J20c, J20f and J13 are killed; J20d and J20e (`at_least(1)` weakened to `at_least(0)`) survive, a small test gap.
- **ulost** (1760-1770). Independent: head and the row at the end are those read when the lease was lost, and the lease is the other holder's. Own word: `lease_lost`, `lease lost`, the other's holder and epoch, `uploaded` unchanged. Plants J26b, J26e, J26f and J26h are killed. Five survive: J26 (`upload_state`), J26c (`lease_holder`), J26d (`lease_epoch`), J26g (head at the end), J26i (the lease at the end). `test_each_courses_files` changes `lease`, `uploaded` and the row only, so the test does not hold that head stayed, which is the course's point; the code does. One limit: the lease is lost with the pool exported and the uploader drained (cut-record.sh:1164-1170), so no commit is under way when it runs out. The course shows that a daemon which has noticed the loss uploads nothing. It does not show that a commit in flight is fenced (G3).
- **ufailed** (1726-1744). The want is `uploaded_whole 0`, `upload_rule delta`, `upload_checked matched` where the second start's log says the intent map was taken up, and a whole cut where it says begun anew. That matches rule 2 and rule 3 as #118's `judge` has them and the description's account of the lead's reading. Which of the two the start should have done is not judged: the upload branch of `main` (cut_daemon_check.py:1990-1995) does not run `instance_lines`, so `intent_state` at the second start is read by nothing. See D1 and D4.
- **urow** (1691-1701). Independent: the row at the end is of the daemon's state and not one the other writer put. Own word: exactly two mismatches and three whole cuts. `upload_abandoned_at_mismatch=at_least(0)` says nothing; it could be left out. J21 (`'2'` weakened), J21d (the state of the row at the end) and J21e (`upload_checked`) survive.
- **ukill** (1702-1712). `upload_found_committed 1` and `uploaded_whole 0` are the daemon's word. Node B would show a second upload of the same cut as "at head under two commits" only if it had fetched the cut under the first commit, and in the run it had not (it fetched 1, 185, 466). J22d and J22e survive: the test does not hold `uploaded_whole 0` or `uploaded` at least 1 for the second instance.
- **ulease** (1745-1759). "Uploads nothing while another holds the lease" rests on the daemon's own `uploaded 0`. `rows.tsv` has the independent evidence (head `none` at every look until the second serve) and the judge does not use it (G4).
- **unostore**, **ukillput**: the status while the store was down and the count after the kill are the daemon's word; the content after is node B's.

### 3. `upload_balance`

**The equation is #118's own contract and I agree with taking it as changes over the last instance. It has not met a real status. Its start look is not taken at rest, which can fail a right daemon (D3).**

- The equation (cut_daemon_check.py:1789-1791) is the one `status.rs` documents for `cuts_replaced` at 8fae8f7. I followed each term in the store: a mismatch's `abandon_all` counts into `cuts_abandoned`, so `upload_abandoned_at_mismatch` is rightly not a term of its own; `cuts_replaced` counts in `freeze` and `flushed`.
- Changes and not totals is right: `cuts` and `cuts_abandoned` are kept across a take-up, `cuts_replaced` and the uploader's figures start at 0 with each instance, and a cut held at the start counts through the change in `cuts_held`. Plant J19h (totals) is killed, as are the plants that drop a term (J19a, b, c, e, f, g), J19i, J19k, J19l and J19m.
- Two plants survive. J19j (`max` to `min`): no test runs `upload_lines` on a recording with two start looks, so "the last instance's" is not held by a test (G6). J19d (`upload_committed_gone` added and not taken away): the test moves that figure by one, which unbalances either sign. The sign in the judge is the document's.
- Only the last instance is balanced. `ufailed`'s first instance has a look at rest (`failed-before.txt`) that could be balanced too; a killed instance has none.
- `rejudge.out`: on every recording made so far the line reads "the status has no cuts_replaced", as it should for e42ea6a.

## Findings

### Blocks the merge

None. The condition in the verdict is a run, not a defect.

### Defects that do not block

**D1. `ufailed` fails a right daemon when node B misses the old state's last cut. Certain as to the judge's result (S3); how often is not known, the one run had about half a second to spare.**
tests/kernel/cut-record.sh:1213-1225 and tests/kernel/cut_daemon_check.py:1641-1646. `rec_ufailed` drains and fails the volume at once; unlike the courses with another writer it does not wait for the watcher (`before_other`, 1022-1036). `daemons_fetches` takes "the k-th state the rows name is the k-th epoch". In run 44255421061 node B fetched one cut of the old state (10) and one of the new (8 of the new state). Cut 10's fetch ended at 446.27 s, the second instance started at 446.59 s and its first cut line is at 446.86 s. If the look falls after the new state's first commit, the new state is the first the rows name, its cut 8 is held to the first epoch's cut line 8, and the content line fails.
S3 takes that recording and removes the one fetch of the old state (the daemon and its uploads unchanged): `upload_content FAIL ... cut 8: held as group 51 (itself), its cut line says group 14 (itself); cut 8 (group 14): 3566 blocks differ`, exit 1. The fix on the rig's side is `before_other failed` in place of the bare drain. The judge could also place a state by the time of its fetches against the start that began the epoch, and call a state with no fetch what it is.

**D2. A `get` that fails every time never becomes the rig's error, the watcher ends with 0, and the judge gives a verdict. Certain (S5). Codex's P2 is the same finding.**
tests/kernel/cut_watch.py:131-141 and 194-197. `node_b` sets `failures` back to 0 on every success, and each `step` begins with a `row` that succeeds. A `get` that ends with 2 at every look (the store erring, the scratch disk full) leaves the count at 1 for the whole recording. At the stop the loop makes three tries and writes `stopped`. S5: 11 failed gets logged, no ERROR line, nothing fetched, exit 0. The judge then says `upload_content FAIL node B found no cut of the daemon's at head`, exit 1, where the module's own text says "ends it with 2, the rig's error". Counting `get`'s failures apart from `row`'s closes it; the loop at the stop should also end as the rig's error when its last try failed. Plants W12, W17, W17b, W18, W19 and W20 survive: `main` (the pause, the fetch after the stop, the exit codes) has no test.

**D3. The balance's start look is not taken at rest. A look that falls between the uploader's count of a released cut and the store's fails a right daemon by one. Certain as a reading of #118 at 8fae8f7; not reproduced; I expect it to be rare. It depends on #118's final shape.**
tests/kernel/cut-record.sh:340-348 takes `status-start-K.txt` as soon as the serve is ready, and `upload_lines` (cut_daemon_check.py:1893-1894) balances from it. In #118 the queue's thread posts the store's figures and then answers the uploader's release (`bins/skyzfsd/src/serve.rs:1157` before `:1175`), so `cuts_held` shows the release at the queue's next turn. The uploader's thread posts `uploaded` or `upload_found_committed` on its own, and the publisher writes the file when woken by a change. `ukill`'s second instance releases the cut it found committed in its first moments: the look of run 44253158409 already has `upload_found_committed 1` and `upload_state reading`. S4, with that run's figures: the whole look balances (`122 = 9 + 115 + 0 - 2`), and the look with the count posted and the release not yet shown gives `122 != ... the right side is 121`, a FAIL.
Under `ukill`'s writer the queue turns at every request and the window is short. I cannot put a number on it without a kernel. Ways out: take the start look until two in a row agree; or have #118 post the store's figures after the desk's answer. The status document says "compare them while the uploader is idle", and the start look does not keep to that.

**D4. `ufailed` cannot tell a first cut that follows the intent map from one that leaves the map's regions out. Certain for the run (`ufailed_map.out`).**
The failure comes after the export and a full drain (cut-record.sh:1216-1218), so nothing is held and head is the volume as exported, less two writes after the last flush. Node B never fetched the new state's first cut; it fetched the eighth, by when those blocks were written again. `ufailed_map.out`: a head built without the map's regions (2 writes, 56 blocks) differs from the reference in 0 blocks at the one cut of the new state node B fetched. So "the first cut is a delta onto the map's base, and right" rests on the daemon's `uploaded_whole 0`, `upload_rule delta` and its log line. Stage 2c met the same thing and added a floor (`carried across failure must show something`). Here the course needs data written after the last uploaded cut and before the failure, and node B's fetch of the new state's first cut before more is written.

### Suggestions and gaps

**G1. Say in the table what the fetches could show.** `upload_content`'s detail could carry, per recording, how many fetched cuts had writes between the cut's instant and node B's read (the last column of the table above), and `life` could hold a floor of one. Today a run in which node B fetched only cuts at rest reads the same as one with power.

**G2. The window between the rig's look and the other writer's act.** `before_other` reads the row, waits for the watcher, and then the other writer acts (cut-record.sh:1041-1050, 1060-1072). A cut the daemon uploads in between fails a right daemon on three lines in `umoved` (S6: content, status, course), and in `urow` puts the other writer's row on a group that is no longer head's, after which the daemon rightly stops uploading. I looked for such a cut in the runs I hold: no cut falls in an idle stretch after a load, about 3 s in each of `umoved` (both runs) and `urow`. The cut 30 ms before node B first saw the other writer's head in 44255421061 is, by its group (three on from the last, as each `nudge_until` round is), the first sync after the act. So it is a risk and not an observation. Exporting the pool before the act, as `ulost` does, would close it.

**G3. `ulost` does not reach the fence.** See question 2. A commit in flight when the lease runs out needs a hook or a writer on the pool during the stop.

**G4. `ulease` could use `rows.tsv`.** Head is `none` at every look until the second serve's start; one line in `course_wants` would make "uploaded nothing without the lease" node B's finding.

**G5. The holder the judge wants.** cut_daemon_check.py:1668 takes the holder from the last line of `rows.tsv`. Where the bucket has no lease that is the word `none`, which is also what e42ea6a's status said for `lease_holder`. S1: no lease in the bucket, `lease held` and `lease_holder none` in the status passes; so does a bucket lease at epoch 7 beside `lease_epoch 1`. Want a holder that is not `none`, and compare the epoch too. J17m (first row for last) survives.

**G6. Tests the stage lacks.**
- No recording with two instances through `upload_lines` (`ukill`, `ulease`, `ufailed` shapes): the last start look (J19j), the offsets across states end to end, the `TWO_STARTS` check with a watcher.
- The default wants `upload_abandoned_at_mismatch`, `upload_given_up`, `upload_committed_kept`, `upload_found_committed` (J17h, J17j, J17k, J17l survive), and a content control that differs (J16b).
- That an `upload_course` FAIL alone ends with 1 (J27 survives: in the one test with a failed course the content fails too), and that a recording outside `TWO_STARTS` with a watcher and two instances is the rig's error (J28).
- `test_cut_watch.py` runs only in `skyzfs-cut-daemon.yml`'s unit job (line 98), not in `ci.yml` and `skyzfs-ci.yml` where the other cut tests run (ci.yml:62-69).
- Node B's new commands have no test against the in-memory store: `lease`'s exit 5 (N07), `move-head`'s group (N08), the row's key in `set-row` and `row` (N09, N10). `ROW_KEY` and `ROW_LEN` are copies of the uploader's, and the two unit tests build the bytes by hand, so a change of the key in #118 is caught by nothing here (N11). Once #118 is in, a dev-dependency on `skyzfs-upload` would pin both.
- The recorder's courses run only on a kernel (R03 to R06 survive by construction).

**G7. The upload stage runs by dispatch only** (`UPLOAD: ${{ inputs.upload && '1' || '0' }}`), and the workflow's pull request paths do not name `crates/skyzfs-upload/`. After #118 merges, a change to the uploader starts no upload course. For the lead to decide, with the cost (MinIO, and every recording once more per image).

**Small things.**
- bins/skyzfs-nodeb/src/main.rs:300: the comment `/// What `get` found.` now sits on `ROW_KEY`; the new code went in between it and `enum Got`.
- `upload_content`'s detail says "N of M cuts written and released", the holder's words; for an upload it is "fetched at head".
- `get` prints the commit it read and the watcher does not use it. Comparing it with the head read before would add a direct check beside the bracket.
- `course_wants` for `ukillput` wants `uploaded_whole=at_least(0)`, which says nothing.
- The new courses' rule controls accept `same` or `caught` (`EXPECTED`), so they pin nothing yet; tighten them once the courses have run a few times.

## What depends on #118's final shape

The rig reads these from the daemon. A change in #118 after 8fae8f7 to any of them changes a verdict here:

- Status keys and values: `upload_state` (`idle`, `no_lease`, `backoff`, `lease_lost`, `off`), `lease` (`held`, `none`, `lost`), `lease_holder`, `lease_epoch`, `uploaded`, `uploaded_whole`, `upload_mismatches`, `upload_abandoned_at_mismatch`, `upload_cuts_lost`, `upload_given_up`, `upload_committed_kept`, `upload_committed_gone`, `upload_found_committed`, `upload_checked` (`matched`, `head_moved`), `upload_rule` (`delta`), `cuts`, `cuts_held`, `cuts_abandoned`, `cuts_replaced`, `cut_state`.
- The balance as `status.rs` words it, and which counts are kept across a take-up. D3 depends on the order of the two posts in `serve.rs`.
- `lease_epoch 1` after a kill and a second start by the same holder (`ukill`, `ukillput`, `ufailed`), and the default holder `<hostname>:<machine-id>`: the rig passes no `--holder`, so a runner without a machine id shows as a refused start.
- Options and exits: `create --endpoint --bucket --volume`; `serve --kill-at-upload swap:3` and `put:5`, `--serve-without-upload`, `--lease-ttl-s 3`, `--acknowledge-failure`; exit 10 for a lease held, 3 after SIGUSR1, 137 at the hook.
- Log lines matched as text: `test hook --kill-at-upload`, `the swap of commit 3 `, `segment PUT 5 landed`, `intent map: taken up`, `intent map: begun anew`.
- The row's key and its version 1 layout, copied into node B.
- After a mismatch with the pool exported the uploader comes to `idle` with `cuts_held 0`; `umoved`'s drain waits for that.
- Until #118 merges, the stage needs `daemon_ref`.

## The plants

`plants.out` has every line. KILLED means a test of the head failed with the plant in; SURVIVED that all passed.

| Where | Plants | Killed | Survived |
|---|---|---|---|
| The watcher (`cut_watch.py`, W) | 24 | 12 | 12: W02a, W02b, W04, W05, W11, W12, W17, W17b, W18, W19, W20, W22 |
| The judge: the daemon's fetches and the content (J01 to J16b) | 17 | 13 | 4: J04, J07, J08, J16b |
| The judge: the status after the drain (J17a to J18b) | 15 | 10 | 5: J17h, J17j, J17k, J17l, J17m |
| The judge: the balance (J19a to J19m) | 13 | 11 | 2: J19d, J19j |
| The judge: the courses (J20 to J28) | 41 | 26 | 15: J20d, J20e, J21, J21d, J21e, J22d, J22e, J24e, J26, J26c, J26d, J26g, J26i, J27, J28 |
| The test of a state through two starts (T) | 2 | 2 | 0 |
| The recorder (`cut-record.sh`, R) | 6 | 2 | 4: R03 to R06, kernel only |
| Node B (N) | 11 | 5 | 6: N03, N07 to N11 |
| All | 129 | 81 | 48 |

T01 and T02 are the two faults the description says `test_a_state_through_two_starts` catches; it does. J07 and J08 survive apart because each covers for the other in `test_what_an_upload_gets_wrong` (a second state's fetch is left out, and the last cut is then missing). W04, W05 and W22 are properties no course reaches. W11 (any failure of `get` taken as a verdict) belongs with D2: no test has a `get` that ends with 2.

## What I ran

- `python3 plants.py <worktree at 32d8d67> plants.out` in three worktrees at once, each plant alone, the file restored after each; the worktrees were clean afterwards.
- `python3 -I scenarios.py <worktree> <run 44255421061's ufailed>`.
- `python3 -I rejudge.py`, `fetch_power.py`, `ufailed_map.py` on the artifacts of runs 44251116091, 44253158409, 44255421061 and 44259950077 (downloaded, read as data).
- `cargo test --locked -p skyzfs-nodeb` (`nodeb-tests.out`).
