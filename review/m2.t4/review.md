## Review of #108 at 6cedb4d (M2.T4)

**Verdict: one thing to fix before the merge (D1), then it can merge.** The releaser does what it says, its tests hold it (11 of my 12 plants are caught), and round 2's rows are what the description says: I read the run's artifact. But a row whose releaser ended early is still labelled "cuts released every N" in the table, and only a warning in the job log says otherwise. Codex raised D1 and D2 as well; I came to both from the code and confirm them.

### The three questions

**(1) Does the releaser stand for an upload's commit, for the intent map's steady-state cost? Yes, as an upper bound, with one figure to put right.**

- **It runs the store's own path.** `release` goes through `release_cut`, which is where #106 commits the map's base, clears the bits and writes the map's pages. Nothing an upload does besides (its reads, its PUTs) touches the map. So for the map, the stand-in leaves out nothing.
- **Its lag is the shortest there can be, which is the map's worst case.** In round 2's log the releaser asked about 8.5 times a second and released within that, so r1 commits a cut about a tenth of a second after its flush. A real upload commits after its own duration, and the longer that is, the more regions have changed since the cut's instant and stay set. r4 lags three cuts. That the two give the same sync counts says the cost is not sensitive to the lag in this range.
- **The figure to put right: "98 releases a row" is the daemon's whole life, not the window.** From `skz-releases.tsv` and the windows' samples in the artifact of run 43726791081:

  | Rows | Releases in all | Before the first window | In the tpcb window (120 s) | In the select window |
  |---|---|---|---|---|
  | r1 (four rows) | 98 | 36 or 37 | 23 | 24 or 25 |
  | r4 (four rows) | 31 or 32 | 11 | 7 or 8 | 8 |

  So the tpcb window saw a commit every 5 s at r1 (one per transaction group) and every 15 to 17 s at r4. The map's syncs in the notes' table are the window's, so 1196 syncs at 4 MiB go with 23 commits, not 98. The notes' sentence should give the window's figures; that is the lead's to change.
- **What it says nothing about is the copies**, as the description states. "Hardly any write had to be copied" is a property of releasing at once, not of an upload, which holds its cut for as long as it runs.
- **The releaser's own load is in every daemon row and in no model row.** Each poll is a SIGUSR2 and a command the daemon answers between turns of its queue (it reads a file, lists each held cut's map to count it, writes and renames a reply). At 8.5 a second that is small beside 6,000 to 7,000 tps, and the same for every daemon row, so the comparisons among them stand. It is a third part of the gap to the model, beside the test-hooks build and the held cuts, which the notes name as not yet separated. A poll interval that can be set (`POLL_S` from the environment, as `REPLY_S` is) would let one row show it.

**(2) The start and the stop, the teardown included. Right, apart from D1.**

- **Start.** Only after the device is ready, so no signal reaches a daemon that has not set up its signals. The stop file is removed first. Stderr goes to the serve log by append, as the daemon's does.
- **A release build survives the signal.** SIGUSR2 is in the set both builds block (`skyzfs-ublk-sys/src/signals.rs:29`), so the releaser does not kill a daemon without hooks; it times out, as its docstring says.
- **Stop.** `skz_stop` stops the releaser before the SIGTERM, waits 30 s, which is the longest one reply may take, then kills it. `skz_stop` is the teardown's too, and `SKZ_UP` is set before every `skz_start`, so no path leaves a releaser behind a stopped daemon. A releaser orphaned by a killed script ends at its next poll, when the daemon's pid is gone.
- **The copy-repeat mode** starts and stops a daemon per repetition; each start writes its own header line, and the handle is cleared at each stop.

**(3) The `+hooks` build. Right.**

- The ref's pattern already allowed `+`, the suffix is stripped before `rev-parse`, and each named ref gets a worktree and a binary of its own. Two entries naming a ref with and without hooks build twice and each binary is copied right after its build, so the shared target directory cannot hand one the other's.
- The feature is asked as the daemon rig asks it (`-p skyzfsd --features test-hooks`). A ref from before the feature fails the step, which is the right outcome.
- The step summary says which builds have hooks, and the row's `build` line carries the `+hooks`. Round 2's rows read `e9281c9…+hooks` and `0c3df3c…+hooks`, and their releases prove the hooks were in.

### D1 (defect; fix before the merge). A row whose releaser ended early is labelled as released

`topology-bench.sh:1940` writes `(cuts released every N)` into the row's label when the copy is done, whatever became of the releaser, and the table shows that label (`:785`, `:826`). A releaser that ends before its stop leaves a last line in `skz-releases.tsv` and a `WARNING` on stderr at the stop (`:1655`). Nothing in the summary changes.

Three ways to get there:

| How | What happens | What the row then measures |
|---|---|---|
| `TB_SKZ_RELEASE` with a ref without `+hooks` | the first `held` gets no reply; the releaser ends after 30 s with exit 2 | round 1's regime (copies up to the bound, cuts abandoned), under round 2's label |
| a `+hooks` build without `--hold-cuts` | `held` is refused ("held cuts are off"); the releaser ends at once | no held cuts at all |
| D2 below | the releaser ends in the middle of a window | part of the window with releases, the rest without |

I am sure of the first two from the code and from `test_main_with_no_daemon_answering`. Round 2 is not affected: all eight rows end with `# released N`.

**A fix.** The sequence's two mistakes can be refused before anything runs, as the script refuses a wrong `--store`: `TB_SKZ_RELEASE` without `--hold-cuts` in `TB_SKYZFSD_ARGS`, and with a `TB_SKYZFSD_REF` that does not end in `+hooks`. And at the stop, write what the releaser did into the row's `.skzinfo` (`releases N`, or how it ended), and show it in the label, so the table and not the log says it. The count in the window would be better still, since that is the figure the notes need (above).

### D2 (defect, small). A cut abandoned between `held` and `release` ends the releaser

`cut_release.py:73`. The releaser asks `held`, picks the cut being uploaded, and sends `release N`. If the store abandons cut N in between (at the bound, or because a copy failed), the daemon answers "cut N is not the one being uploaded" and the releaser raises and exits 2. The cut rig's holder takes exactly this case as not its error and goes on (`cut_hold.py`, `must_be_gone`). The window is one poll wide and needs the bound to be reached, so it is unlikely in rounds like round 2, where nothing was copied. It matters more for rounds meant to have copies and releases together. With D1 fixed it would at least show.

**A fix.** After a refused release, ask `held` again: if cut N is no longer the one being uploaded, go on; if it still is, that is the error.

### G1 (gap, small)

A refused `held` taken as "nothing held" passes every test (my plant P6). That is the second row of D1's table: a daemon without `--hold-cuts` would then be polled for the whole row with nothing ever released and exit 0. One test with a fake that refuses `held`.

### The two gaps from #103's last look (6cedb4d)

Closed. My two plants that passed there (a write with no hash for a block does not count; every block compared with the write's first hash) now fail `test_copied_blocks`, and the other two still do (`plants4_6cedb4d.out`).

### My plants in the releaser

`review/m2.t4/plants.py`, results in `plants.out`: when a cut is due (four ways wrong), a refused release counted as one, the exit code, the error's line, the end's line, a release's line, the stop file and the reply limit are each caught. P6 is G1.

### CI and Codex

- `ci`'s lint job, which runs both changed test files, is green on 6cedb4d; `test`, `unit-arm64` and `coverage` were still running when I wrote this. Testing cancelled the rigs that the one changed file under `tests/kernel/` had started, and says so on the pull request; none of them runs a file this pull request changes. Whether cancelled counts as green is the lead's, as on #103.
- Here: `test_cut_release.py` 8 tests and `test_cut_daemon_check.py` 58 tests pass. I have no shellcheck in this container; CI's lint ran it.
- Codex has two open P2 findings, which are D1 and D2.

### Files

`review/m2.t4/` on `skyzfs-review`: `review.md`, `plants.py`, `plants.out`, `plants4_6cedb4d.out`.
