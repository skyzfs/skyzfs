# M1.3ac part 2 (#77): one commit per request, checked at the hand-over

Review posted at 14df6f7 (issuecomment-6872970565). One blocking finding,
a missing test: the failure path's two resets of a Waiting slot to Idle
(pump.rs:543-545 in fail(), pump.rs:603 in the failed turn) are held by
no test (plants P2, P4 pass everything). repro77.patch has the test that
holds both (passes unplanted, fails with each). Nothing else blocks;
QueueDown is right; remark for the lead on the daemon's words at a
QueueDown. Suggestions: P1 (range check in the failed turn) untested;
caught() accepts the rule alone.
- `look2-posted.md`: the second look at b02f876 and 6ff9b08
  (https://github.com/skyzfs/skyzfs/pull/77#issuecomment-6873256177):
  the blocking test is in; nothing blocks.
- `gates77c.txt`, `plants77c.py`, `repro77c.txt`: gates and plants at
  b02f876, where P2 and P4 still passed every test and the reproducer
  failed with each.
- `gates77d.txt`, `plants77d.py`: gates and plants P1-P8, Q1-Q5 at
  6ff9b08; all caught.
