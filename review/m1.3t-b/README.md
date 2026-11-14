# M1.3t-b (the acknowledged-position plumbing removed), pull request 74

Head e41e193 on 09959c2. Review posted on 2026-11-14 (issue comment
6871721214): nothing blocks; it can merge once the rigs are green.
`review-posted.md`.

- `tests-gone.txt`, `tests-new.txt`: by `../m1.3t/static-tests.sh`, 24
  tests go and 3 come (coding's 124 to 103). Each gone test read
  against the base before the description; all hold the record, its
  retries or positions.
- `gates.txt`: fmt, clippy (plain, test hooks, control-test-program)
  and the tests of skyzfs-ublk, skyzfs-local and skyzfsd: clean, rustc
  1.100.0.
- `plants74.py`, `plants74.txt`: L1 (note read before the lock) 29
  fail, L2 (acknowledged start keeps the note) 3, L3 (acknowledging
  start opens without the flag) 7. D1 (the delayed queue's flush
  without release) fails nothing and cannot: `wait` gives all on when
  the stop arrives. E1 and E2, experiments: with a lasting marker,
  and with an acknowledging intruder too, UNLOCK_BEFORE_FAILED is not
  noticed. The one-shot marker stands for a failure the next open does
  not repeat, which no store gives since M1.3t-b.
- Told the lead: "delete under the lock" is now a guard; #74 keeps
  `unstored` for a reader that #73 removes.
- The first plant run misread cargo's "test failed" as a build error
  and ran D1 on the binary target (no tests); fixed and run again.
- To do: the daemon rig's tables on this head; Codex.

## Second look at 952c695 (posted: issuecomment-6872115861)

Blocking: does not build on today's base (a7853c9, #71 merged). #71's
control program implements `Queue::pause` (skyzfs-ublk-control.rs:868),
which this pull request removes from the trait. Textual merge clean;
ci.yml lint/test/unit-arm64 fail with E0407 (run 43556985568). Fix: merge
the base, drop the method. 952c695's doc commit answers point 2.
