# M1.3t-d (#79): serve has no --store

- `review-posted.md`: the review posted on #79 at head ba29846
  (https://github.com/skyzfs/skyzfs/pull/79#issuecomment-6873119309).
  Verdict: can merge once CI and the daemon and replay rigs are green;
  nothing blocks, no defect.
- `gates.txt`: fmt, clippy and the skyzfsd tests at ba29846, plain and
  with test-hooks, in a worktree with its own target dir.
- `plants79.py`, `plants79.txt`: five plants (T1-T5) in `cli.rs`, each
  alone, and the cli tests after each; the new test catches all five.
