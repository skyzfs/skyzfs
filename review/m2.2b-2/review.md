## Review of #101 (M2.2b-2: the rigs' interface to held cuts) at 7e4b7b8

**Verdict: nothing in M2.2b-2's own commit (ee01a8d) blocks the merge.** I found no defect in it. There are three test gaps and a few remarks.

The branch holds #100, so #100's D1 and D2 are in it, and it merges after #100's next head. Codex's P1 here (`serve.rs:747`) is #100's D1.

- **S1:** a write's hashes away from the volume's labels have no test.
- **S2:** no test turns the commands with no signal.
- **S3:** `serve`'s own lines for the commands and the status have no test. This one is for the rig.

CI is green at 7e4b7b8: 36 check runs, 34 success and 2 skipped.

The files named below are in `review/m2.2b-2/` on `skyzfs-review`.

### How I checked

- **The code first**, against the module's protocol: `cut_commands.rs`, `serve.rs`, `record.rs`, `signals.rs` and `status.rs`, before the description.
- **The tests pass here** (rustc 1.100.0): `skyzfsd` with its test hooks, the status tests of `skyzfs-ublk` and the signal test of `skyzfs-ublk-sys`.
- **Planted changes.** 30, one at a time (`plants.py`; `plants.out`, `plants_gvw.out`, `plants_r7.out`). The tests caught 23. Of the 7 that passed:
  - one is S1;
  - one is S2;
  - four are S3;
  - one is in an arm I found no way to reach (small remarks).
- **Three tests of my own** (`gaps.py`, `spans_turns.py`), each run at the head and under a plant.

### Test gaps (none blocks)

**S1. A write's hashes away from the volume's labels have no test.** (`record.rs:535`; the test at `record/tests.rs:618`. Confidence: high, run here.)

- The test's two writes are at sectors 8 and 9, inside the first two labels, where the record writes a `label` line as well.
- With the hashes kept only where that line is written, every test passes. Any other write then has `-` for its hashes and no `data=`. That is nearly every write of a pool, and it is what M2.T3 rebuilds the volume from.
- `gaps.py` puts one write in the middle of the volume through the record. It passes at the head and fails under the plant: the line ends `-` where the hash and `data=0` belong.

**S2. No test turns the commands with no signal.** (`cut_commands.rs:246`. Confidence: high, run here.)

- With the command read at every turn that has no write under way, every test passes.
- In the daemon a `release` answered `ok` would be answered again at the next turn, as refused, under the same token. A `write` would begin again each time it ended.
- `gaps.py` lets a cut go and turns once more with no signal. At the head the reply stays `s ok`. Under the plant it is `s refused: cut 3 is not the one being uploaded`.

**S3. `serve`'s own lines have no test.** (`serve.rs:590`, `:953` and `:1198`. Confidence: high, by reading: nothing here runs `serve`'s `run`.)

- Four plants in `serve.rs` pass every test:
  - a state taken up is shown as begun, in the status and in the request log's `cuts` line;
  - `cuts_held` shows the copied bytes;
  - the flag is read and not cleared, so every turn after one signal reads a command;
  - `SIGUSR2` sets no flag, so no command is ever read.
- This is for M2.T3's rig. Any use of the commands shows the last two. The first two need a line each: `cut_state` and the `cuts` line after a take-over with `--hold-cuts`, and `cuts_held` against the lines of `held`.

### Your three questions

1. **The command protocol.** It is as the module says, and I found no hole in it.
   - **Busy.** A command that comes during a write is refused, and the write goes on.
   - **A reply replaced.** A reply is written whole and renamed. A busy reply is replaced by the write's when the write ends, which can be in the same turn. The sender of the second command then waits for a token that never comes. The module says the rig waits for each reply, and the token is what lets it tell whose reply it reads.
   - **A write refused when its map changed.** The check is by the map's length, and that is enough: a held cut's map only gains blocks, from an older cut abandoned at the bound, so the same length is the same map. A cut that goes under the write is noticed at the next read. The last read and the reply are in one call, so nothing comes between them.
   - A cut let go as uploaded, under a write of the cut that waits, changes neither that cut's map nor what it reads. The write is right: PATH is then the image of the cut just uploaded.
2. **`write` reads each block as the cut reads it.** Yes.
   - A block changed since the instant is read from its copy, and any other from the home image. No write to a block read from the image is under way then:
     - a write received before the cut's flush has ended, since the flush waits for it and a cut whose flush is under way is refused;
     - a write received after it made its copy before it began (`note_change` in `execute`), and the state then says "copy".
   - The pull request's store test has a volume of one slice. `spans_turns.py` does it over four slices, with the client overwriting four blocks between the first slice and the second: one already in the file, three not. All four are in the file as at the instant. With the copy before a write planted out, the three read as the client wrote them.
3. **`SIGUSR2` in every build.** Yes.
   - It is in the blocked set, and `wait` names it. `serve`'s loop is the only reader besides the control program, which acts on a stop alone.
   - The default build sets a flag that nothing reads and wakes the queue once.
   - With the signal left out of the set, the test's process ends, as the description says. With it taken for `SIGUSR1`, the test fails.

### Also checked

- **Status v13.** The layout, a version 12 file and the keys `status` prints each have a test that fails under a plant. The figures are bytes. `skz_time.py` and the rigs read the text of `status` and compare no version for equality.
- **The request log and #99.** #99's check at 6e0ecd8 already takes eight to ten columns, reads the cut's number from the sectors column and passes over the `cuts` line. The two fit, whichever merges first.
- **The hashes.** The first 8 bytes of SHA-256 for a block covered whole, and `-` for one covered in part. Both ways of getting that wrong that I planted are caught.

### Small remarks

- **An arm I could not reach** (`cut_commands.rs:390`): a block that is in the write's map and not in the cut's. Writing the buffer there all the same passes every test. A held cut's map loses no block, and a map of another length is refused at the write's end anyway.
- **`cuts_abandoned`.** `status.rs` and `state.rs` say it counts the cuts abandoned at the bound. Since #100 it also counts the cuts let go at a refused copy.
- **After the volume has failed** the commands still answer, from a state that design §5.2 no longer trusts. This is by reading. A course of the rig that fails the volume should not write a cut after it.
- **A comment.** `cut_commands/tests.rs:269` says three slices and `:279` two more. 350 blocks are two slices and one more turn, which is what the assertion holds. The description's "after its third slice" is the same slip.
- **The device waits during a slice.** A slice is 256 reads on the queue's thread at each turn of a write. That is fine for a rig.
