## Review of #106 at 7729360 (M2.3b)

**Verdict: not yet. Two defects, and I would fix both before the merge (D1, D2). Neither makes a wrong cut.**

- **D1:** the start that creates `intent.map` does not sync the directory, so a power cut can lose the file's name, and the next cut is then the whole volume.
- **D2:** Codex's P2 is right: a store dropped while a write waits for the map does not end, or drops the write unwritten.

The rest reads right to me: the wait and its ordering, the clear, the start and the trust of the map, and status version 14. Three test gaps matter (G1 to G3).

I reviewed 21f6f83. The head is now 7729360; since 21f6f83 it changes only `intent/model.rs` (merged with #105) and the notes, so the plants below, run at 21f6f83, stand.

### D1: the new map's file name is not made durable

- **Where.** `inplace.rs:2369`, `keep_intent`: the old file is removed, a new one is created, written and synced, and `Dir::sync` is never called. `Dir` says a creation is durable only after `Dir::sync`, and the open and the volume's own files keep to that (`volume.rs:95`, `start.rs:404`).
- **Scenario** (`name_not_durable.py`, `name_not_durable.out`). A new volume's first start begins a map. Cut 1 is committed, and block 150 is written with its region's bit set and synced. The power is cut; every sector keeps what it was last written with, and only the creations no directory sync covered are lost.
- **The wrong result.** The start after it says `Begun("there is no intent map")`, and the next cut is the whole volume. With one line added (`self.dir.sync()?` after the creation) it says `TakenUp { named: 1, regions: 4 }`.
- **What it costs.** No block is lost. It costs the upload the map is there to save, in the case the map is for. On ext4 and XFS a sync of a new file usually carries its name with it, so the rigs may never show it; the design does not rest on that elsewhere.
- **It also hides the map from the power-cut test.** Of the 1797 starts after a power cut in `after_a_power_cut_the_next_cut_holds_every_block_changed_since_the_newest_commit`, 768 find no map at all (`narrowed_count.py`, `narrowed_count.out`). Confidence: high, by a run.

### D2: the store's drop while a write waits for the map (Codex's P2)

- **Where.** `inplace.rs:891`, `Drop`. The loop goes on while a request is on the threads or deferred, and only collects the threads' completions. Nothing in it looks at the map's sync.
- **Shown by a run** (`drop_while_waiting.py`, `drop_while_waiting.out`), with I/O threads, after the disk has let the sync end:
  - a write waits for the map, and a read of its block waits behind it: the drop did not end in 15 s;
  - the write waits alone: the drop ends, and the image was never written. The comment there says what was submitted is done before the store goes.
- **Where it bites.** A daemon that ends with requests in flight, as after a refused commit or fetch, drops the store so, and can hang while it holds the volume's lock. Confidence: high for the store, by a run; I did not drive the daemon there.
- **A fix.** In the drop's loop, also settle the map's sync (waiting for it), and go on while a request waits for the map.

### The four things you asked about

1. **The wait off the queue's thread.** Right, apart from D2.
   - A waiting request counts as a write in flight, so a request that overlaps it and a flush wait, and what comes behind a waiting flush waits with it. Nothing else waits.
   - `covers` and the change in the cut state are one step, in `execute` and again in `intent_synced`, with no clear between.
   - A failed write of a page, or a failed sync, fails the volume and every waiting request before any of them begins.
   - The pages are written on the queue's thread before the sync begins on the helper, so the two rules M2.3a gives the caller hold.
   - Planted wrong, seven of these fail a test (below). `INTENT_RELEASED_UNCHECKED` is the right mutant for the release.
2. **The three points under (b).** Each has its test, and each fails when the rule is planted away: a start that acknowledges a failure does not take the map up; the map judged by this start's own token; a start whose write or sync of the map fails serves all the same.
3. **The lead's reading of "state file".** The power-cut test leaves the cut state gone, as it was, or as it was at an earlier group, and requires that none is taken up. That is the reading. `BOOT_UNCHECKED` is caught by that requirement.
4. **Status version 14.** The layout is right (352 and nine fields are 424 bytes), version 13 is still read, and the keys are printed. Three plants in it fail the status tests.

### What I checked

- **The tests pass here on 21f6f83:** `skyzfs-local` with its test hooks, 233 with 1 ignored, and 13.
- **20 plants in the store** (`plants.py`, `plants.out`): 15 fail a test (one of them by a test that does not end), 5 pass (G1 to G4).
- **7 plants in the status, the daemon's start and the options** (`plants_status.py`, `plants_status.out`): 5 fail a test, 2 pass (G4).
- **How much the power-cut test narrows** (`narrowed_count.out`): of 1797 starts after a power cut, 405 take a map up. It names every region in 261 of them and fewer in 144.
- **CI:** green on 21f6f83 (34 passed). On 7729360, 28 passed and 3 still ran when I wrote this; none failed.

The files are in `review/m2.3b/` on `skyzfs-review`.

### Test gaps (none blocks by itself)

**G1. No test has a write-zeroes ask the map.** With the write-zeroes going on without the map (the three lines taken out of `execute`), every test passes. Half of "set before written" is unheld. A write-zeroes step in the store-level programs would hold it.

**G2. No test has a flush alone behind a write that waits for the map.** With a flush looking only at the threads, every test passes. In `a_write_into_a_clean_region_waits_for_the_map_and_no_other_request_does` the flush waits because the read before it waits. A flush submitted with nothing deferred before it would hold the rule.

**G3. No test starts twice on a map that was taken up.** With `kept_by` taken out for a map that is taken up, every test passes. The map then stays under the old token, and the start after the next one begins a new map. A second power cut in the store-level test would hold it (the review of #105 asked for one).

**G4. Small ones.** Each passes every test:
- a failed write of the map at a release does not fail the volume;
- a write that waited goes on though the volume failed after its bits were found durable (another write of the same batch failed first);
- `--intent-region-mib` is parsed and not handed to the store;
- a start that began a new map shows 0 regions named at the start.

### Suggestions

**S1. The power-cut test narrows in 144 of 1797 runs.** Two things keep it low:
- D1 takes 768 runs;
- the volume has four regions and each holds a label, so every group names all four. A volume with more regions than labels, and D1's fix, would put the map to work in most runs.

**S2. `intent_figures` builds the list of every named region to count it** (`inplace.rs:2582`, `named().len()`), each time the status is written. Before M2.4 nothing is committed, so every region is named. At 1 MiB regions on 1 TiB that is a million entries each time. A count of the bits would do.

### For the lead and testing

- **The refusal for a file system with no direct I/O comes after the map is written** (`inplace.rs:1102` and `:1106`). A start refused there with `--hold-cuts` has created and written `intent.map`. The daemon rig's pin on a refused start allows the status file and `volume.start` to change; it does not start with `--hold-cuts` today. Where the direct write of the map is itself refused, the start says "the intent map: …" and not that the file system offers no direct I/O.
- **Before M2.4 only the test-hooks build ever commits a cut.** In the plain build every region stays named, no write waits, and every start begins a new map ("no committed cut"). So M2.T4's cost of the map's sync can only be measured with the test-hooks build and the holder's releases.

---
_Generated by [Claude Code](https://claude.ai/code)_
