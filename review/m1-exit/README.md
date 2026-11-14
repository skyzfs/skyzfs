# The M1 exit round (notes 8697790), checked against its runs

- `look-posted.md`: what was posted on PR #2.
- `recompute.py NOTES`: recomputes every cell from the per-row tps in the
  notes' table, and again classed by sync rate (the rule as fixed before the
  round). Output in `recompute.txt`.
- `parse_bench.py JOB RUN SUMMARY`: reads one bench job's summary.md (machines,
  builds, serve arguments, tps per row, each daemon row's verdict line).
  `bench-parse.txt` holds its output for all thirteen jobs.
- The artifacts themselves are not kept here; they are on the runs named in
  `look-posted.md` until they expire.
