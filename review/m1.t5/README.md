# M1.T5 (skyzfs/skyzfs#12) at baa08d3: review notes

- `replay-failed-log.tsv`: four lines of trace, as the tool writes them
  for a write that is in flight when the log is made to fail and comes
  back with the kernel's error. `skyzfs-blkcheck --replay
  replay-failed-log.tsv 7` prints
  `UnexpectedError { id: RequestId(1), error: Io(Other) }`, 1 violation.
  The live run has none: the tool turns that error into "the volume has
  failed" after it has written the `done` line, and the suite's model is
  given the turned result. With `Err(Failed)` in the `done` line the
  replay is clean.
- Planted in `bins/skyzfs-blkcheck/src/device.rs`, 7; noticed by the unit
  tests, 4 (results returned before the kill kept as carried; the detector
  comparing the wrong way; the pid signalled whatever process it is; the
  failed flag set after the helper). Not noticed, 3: `settle` never asks
  the detector; the time of the sorting is not kept (the detector then
  never fires); the sorting is not done at the helper's `dead` line. The
  last would show in a kernel run (no `sorted` line, so the new pin is a
  rig error); the first two would show nowhere.
- The judge at baa08d3 answers exit 2 to all six inputs of
  `review/m1.t4/judge-holes.py`. Still exit 0: an observation that says
  "Not run" or "was not run" (only the prefix "not run" is looked for; the
  rig writes that prefix everywhere today); a pin of a check that was not
  asked for, whatever it says.
