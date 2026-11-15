## Review of #103 at 718d198 (M2.T3 stage 2a)

**Verdict: the review says it can merge. Nothing blocks.**

I reviewed d905bd4 as asked, and then 718d198, which the pull request now has. I found two defects in how the rig attributes a failure. Both are rare, and neither hides a wrong daemon:

- **D1:** a write-zeroes received while a flush waits makes the judge fail a correct daemon.
- **D2:** the holder releases a cut whose write was refused for the rig's own reason, and the judge then blames the daemon.

Besides: test gaps (S1 to S3) and two suggestions (S4, S5).

### Your three questions

1. **The content model.** Right, except for D1.
   - Playing `records[:prefix]` is the cut's instant for content. A write is placed at its `applied` line, and the store applies everything received before a flush before that flush's barrier, and nothing received after it.
   - The runs agree: every written cut is compared over the whole volume (131072 blocks a cut), not only over its map.
   - `apply`, `same` and `held_compare` do what the description says. Planted wrong one at a time, 8 of 11 changes in them fail a test (S2 and S3 have the rest).
2. **The holder's protocol.**
   - One signal per command: right, for the reason you give.
   - Token matching: right. The space after the token keeps `t1x1` from answering for `t1x10`, and a test holds it.
   - A write refused at its end is left unrecorded: right where the cut has gone. But the release follows every refusal (D2).
   - The drain: right. It ends with `held` empty or with the holder's error.
3. **Whether a wrong daemon would be seen.** Yes, and I measured it on the run's own files (below). The two controls show that the comparison is not empty for the middle cut. They do not show a daemon that makes no copy, so I tried that directly.

### What I checked

- **The unit tests pass here:** `test_cut_daemon_check.py`, `test_cut_daemon_main.py`, `test_cut_hold.py`.
- **The judge gives the same verdicts here.** I ran it on the ten recordings of run 43701571141 from its artifact: exit 0 on all ten, with the counts of your table.
- **A daemon that makes no copy is caught on real recordings** (`no_copy_daemon.py`, `no_copy_daemon.out`). I rewrote the holder's hashes as such a daemon would have left them: a block that a later write changed before the write command was sent gets that later content.
  - `config`, 8 blocks in one cut after the first: `held_content` FAIL, exit 1, naming cut 18.
  - `multihost`, 25 blocks over 20 cuts, one or two a cut: FAIL, exit 1.
  - `life`, 1751 blocks of the first cut: FAIL, exit 1.
- **The recordings do read cuts from copies** (`copies_shown.py`, `copies_shown.out`). For each written cut I counted the blocks of its map that a later write changed before the write command was sent. The daemon read those after the overwrite, so what it wrote came from a copy.

  | recording | in the first written cut | in later cuts |
  |---|---|---|
  | life | 1751 | 0 |
  | slog | 212 | 0 |
  | rewind | 410 | 12 |
  | rewindsync | 1376 | 8 |
  | rewindf | 0 | 12 |
  | multihost | 1809 | 25 |
  | config | 125 | 8 |
  | stray | 3137 | 4 |
  | autotrim | 3406 | 0 |
  | full | 4235 | 67 |
  | bound (run 43702505054) | 21145 | 0 |
  | edges (run 43702505054) | 1756 | 4 |

  Nearly all of it is the first written cut, whose map is the whole volume (S4).
- **Planted changes** (`plants.py`, `plants2.py`; `plants_718d198.out`, `plants2.out`): 59 in the judge and the holder, one at a time. The tests caught 40. Of the 19 that pass, 3 change nothing a run would see.
- **CI on 718d198:** 28 checks passed, 21 were cancelled, none failed. The cut rig on the daemon passed on both images (run 43702505054). The cancelled ones include the daemon rig and the replay rig, which this pull request's files do not touch; whether that counts as green is the lead's, as on #99.

The files are in `review/m2.t3-2a/` on `skyzfs-review`.

### Defects (neither blocks)

**D1. A write-zeroes received while a flush waits is placed before that flush's barrier, and a correct daemon fails.** (`cut_daemon_check.py:223` to `:231`. Confidence: high for the judge, by a run; for the store, by reading.)

- The judge places a write-zeroes at its receipt. The store puts whatever it receives while a flush waits behind that flush (`inplace.rs:1587`; design §5.2, "The instant is a flush's barrier"). So such a write-zeroes takes effect after the barrier, and the cut holds the block as it was.
- **Scenario.** A write of block 30 is in the store. A flush that marks a cut is received and waits for it. A write-zeroes over block 10 is received. Then the write is applied, and the barrier and the cut follow. A correct daemon's cut holds block 10 with its old content.
- **What the judge says** (`zeroes_behind_flush.py`, `zeroes_behind_flush.out`): `held_content` FAIL, "block 10: zeros wanted". The block is not among those it calls unplaced, because no pending write overlaps it. So the line is a verdict against the daemon, exit 1.
- **How likely.** It needs a write in the store when the flush arrives, and the write-zeroes within that wait. `edges` sends one write-zeroes a run, so it is rare. When it happens it reads as a daemon failure.
- **A fix.** While a flush has been received and its barrier has not come (`received` is not empty), place the write-zeroes after that barrier, or count its blocks as unplaced.

**D2. The holder releases a cut after any refused write, and a cut still held is then let go unwritten.** (`cut_hold.py:207` to `:210`. Confidence: high, by a run.)

- The daemon refuses a write for more reasons than the cut having gone: it cannot open or write the rig's image or its `.blocks` file (`cut_commands.rs`, `begin` and `slice`). The cut is then still the one being uploaded, and `release N` succeeds. The comment says the release tells whether the cut is still held, but nothing reads its answer.
- **Scenario.** The runner's disk is full when the daemon writes `held.img`. The write of cut N is refused with the I/O error. The holder releases cut N, logs no error and goes on.
- **What follows** (`refused_then_released.py`, `refused_then_released.out`).
  - Against your fake daemon, changed to refuse so and keep the cut: the holder wrote cut 2 only, nothing was left held, and it raised no error.
  - On `config`, with the second written cut taken out of the holder's files as if this had happened: `held_content` FAIL at the next cut, 7013 blocks, exit 1. That is a verdict against the daemon for the rig's own failure.
- **A fix.** Release only after a write that was answered `ok`. After a refusal, ask `held` again: if the cut is gone, go on; if it is still being uploaded and the refusal was not about the cut, that is the holder's error.

### Test gaps (none blocks)

**S1. Nothing holds the second stage's exit code.** Eight plants pass:

- a failed `held_content` is no verdict (the line still reads FAIL, and the judge exits 0);
- a control that differs from its expectation is no verdict;
- a control with nothing wrong counts as caught;
- the second stage's lines are left out of `main` though the holder ran;
- the recording's name is not passed to the second stage, so `bound` and `edges` are judged as any other;
- a wrong block under an unplaced write-zeroes is a verdict, and the reverse, every wrong block is the rig's error;
- a refused `held` at the stop is not the rig's error.

`HeldLines` looks at the lines and not at what `held_lines` returns for a wrong content or a missed control, and no test runs `main` on a directory with the holder's files. The code is right today: on the rewritten recordings above the judge exits 1. One test through `main` with a wrong block, and one with a control missed, would hold most of these.

**S2. The comparison's reach has no test.** With only the blocks the image holds compared, or only the blocks the reference wrote, every test passes. The first misses a block that no map ever held. The second misses content the daemon put, inside a map, into a block that was never written.

**S3. Small ones.** Each passes every test:

- a block discarded in part stays known to the model;
- the held answer (`itself` or `rewind`) is not compared with the cut line, only the group;
- `edges` with a write-zeroes and no discard, or the reverse, is no error (the test has neither);
- only five wrong blocks are kept for the unplaced ones;
- two signals for each command (the fake daemon does not answer busy).

Three more change nothing a run would see: a reply read by its first line, the cut that waits written when none is being uploaded, and the command written in place.

### Suggestions

**S4. Have the judge count the blocks that were read from copies.** The table above is not in the rig's output. Today the first written cut maps the whole volume, so every recording has hundreds of such blocks. In later cuts there are few: none in life, slog and autotrim. M2.3 is to narrow what a new state's first cut counts. After that, several recordings may read nothing from a copy, and `held_content` would pass on a daemon that makes none. A line with the count, from the request log's times and `held.tsv`'s (which the judge does not read yet), and a floor for it where one is expected, would show that when it comes.

**S5. `edges` shows where the requests end, not that a held cut keeps what a write-zeroes replaced.**

- In run 43702505054 the holder wrote cuts 1 and 16. Cut 1's instant is before the first direct write, and cut 16's is after the last. No written cut has its instant between the write and the write-zeroes.
- So a daemon that zeroes a block of a held cut without copying it first would pass. You name part of this under "Not done".
- The holder's pause would do it: let the holder drain, pause it, make the write and a cut, send the write-zeroes, the discard and the one-block write, then let it go on. That cut must read the first write's data where the zeros now are.

### Smaller remarks

- **A request log with no hashes passes `held_content`.** I stripped them from `stray`'s log: the line read PASS with 10716 blocks "not known", and the run failed only through `ctl_content_stale`. The text calls every unknown block "a write that covers part of a block", which is wrong there. A write with no hashes at all could be the rig's error.
- **The description's commit range** reads `d905bd4..718d198`; the pull request is those two commits.
