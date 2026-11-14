# origin_pointer_lifecycle at api.rs:1206: a reading

Read at `zfs-topology` 34d507a. The failure is coverage.yml run 43599052029,
attempt 1, job 132459696555, on #86's head 881fe80.

## Verdict

**A defect in the kept SkyNBD code, not a race in the test.** High
confidence: the code and the log leave no other path, and a planted
delay reproduces it.

## What failed

The log reads `api.rs:1206:5`, `left: String("snap/g1")`, `right: "snap/g2"`.
Line 1206 is `body["served"]`. Lines 1203 to 1205 passed:
- the row named `snap/g2`;
- `initialized` was true.

So the export's engine served g1 while the row named g2.

The test's own check at api.rs:1163 had passed: the export's engine
head was g2 right after the forward repoint. So the export went from g2
back to g1 **after** the POST had returned.

Between 1163 and 1202 no API call swaps an engine:
- `snap/nope` returns 404 and `head` returns 400 before any repoint;
- creating `golden` swaps nothing;
- the origin POST naming `snap/golden` returns 400 at `repoint_forward`.

## How the export goes back

- **The sync task reconciles on every snapshot ref.**
  `crates/skynbd-nbd/src/sync.rs:60-65` calls `origin::reconcile` on every
  `snap/` ref event. Here the event is g2's creation, at api.rs:1139.
- **`reconcile` reads the row, then swaps later.**
  `crates/skynbd-nbd/src/origin.rs:161-183`:
  1. it reads the row (`resolve`, origin.rs:63-75);
  2. it calls `repoint` (origin.rs:101-130);
  3. `repoint` compares the export's commit and opens the engine with an await;
  4. only then does it swap.
- **Nothing orders it against `set_origin`.** `set_origin` commits its
  transaction, then calls its own `repoint`
  (`crates/skynbd-api/src/lib.rs:1112-1118`).

The interleaving:
1. The sync task reads the row: g1.
2. The API commits the row g2 and swaps the engine to g2.
3. The sync task compares: g2 is not g1.
4. The sync task opens g1 and swaps it in.

**Nothing undoes it.** Pin and meta writes emit no watch events: in
`MemoryRefStore`'s transaction, only `CasRef` pushes an event. So
nothing reconciles again until the next `snap/` event. Here that is
`golden`'s, and its reconcile had not run by the final GET.

## In the product

The same sync task serves each volume (`provision.rs:869`). When the
race is lost:
- **`<vol>/origin` serves an older generation than its row names.**
  That breaks Invariant E: what is served moves only forward.
- **That generation is unpinned and can be deleted while it is served.**
  The `origin` pin moved with the row (lib.rs:1103-1108). That breaks
  Invariant D: the pointer pins what it serves.
- **It lasts until the next `snap/` event or a full resync.** Waiting
  does not heal it.

**Other paths with the same check-then-swap, read but not run:**
- two concurrent POSTs: both transactions commit in order, but their
  swaps can land in either order;
- the serve path's synchronous reconcile (provision.rs:840, 1024)
  against a POST.

## The test is right

The test does wait for the POST's effect:
- the POST's answer said `served` g2;
- its check at line 1163 saw g2.

Polling the GET until it reads g2 would hide the defect.

## Reproduced

`origin_race_repro.rs.txt` is a scratch test: copy it to
`crates/skynbd-api/tests/origin_race_repro.rs`. `plant.py` adds a
100 ms sleep in `reconcile` between `resolve` and `repoint`. The sleep
only widens a window between two awaits of a task that runs in parallel
with the handler.

| run | result |
|---|---|
| single-shot, no plant: repoint g2, then GET 500 ms later | passes |
| single-shot, planted | **fails**: the repoint answered `served` g2, and 500 ms later `served` reads `snap/g1` (CI's shape, not healed) |
| 200 rounds of new generation, repoint, GET 20 ms later; planted | 42 of 200 stale, e.g. "g11: served snap/g6" |
| the same with no plant, 200 rounds; and 2000 rounds on one CPU | 0 stale |
| the original test, no plant, 300 runs on one CPU | 0 failures |
| the original test, planted | passes: the late swap lands after the test ends |

The natural window is narrow. The coverage build is instrumented and
slower, and it hit the window once.

## A fix direction, for whoever is assigned

Serialize the origin export's swaps per volume:
- **One mutex per volume** (or per export), held from reading the row to
  the swap, in both `reconcile` and `set_origin`'s repoint.
- **Re-read the row under that mutex,** and swap to what it names then.
- **Keep the engine's open outside the export's `RwLock`,** as the
  module doc wants.

A test can then show it without timing: hold that mutex, let a
reconcile read the row, then repoint and release.
