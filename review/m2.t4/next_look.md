## Next look at #108, head 775814c

**Verdict: can merge once CI's last jobs on 775814c are green. D1, D2 and G1 are fixed, and nothing is open from the review.**

I read the diff since 6cedb4d before the answer, ran the four changed test files here (13, 14 and 5 tests, and `test_skz_store.sh`: all pass), and planted each fix back (`review/m2.t4/plants2.py`, results in `plants2.out`).

### What I was asked to judge

**Reading the releaser at the two snapshots and not at the stop: the right place.**

- The two readings enclose the window, so "ended before the second reading" is exactly "did not release for the whole window", which is what makes a row not the row its label says. The stop comes at the teardown, after the row's JSON is written, and could only say it too late.
- A releaser that ends between the two windows of a row marks the second window and leaves the first its verdict, which is right.
- **The line itself works.** No test runs the shell line that makes the reading (below), so I ran it as the script has it (`review/m2.t4/reading_line.sh`, `.out`), with a releases file holding a header, two releases, a gone line and an error line, and a background child as the releaser:

  | The releaser | The line |
  |---|---|
  | running | `skzrel 2 1 # … cut 4 gone after its release was refused (…)` |
  | ended, not yet waited for, while the shell ran a foreground command | `skzrel 2 0 # ERROR HolderError: no reply …` |
  | never started | `skzrel 2 0 …` |

  So an ended child reads as ended without a `wait`, and only release lines are counted.

**The gone-cut line: fine.** It begins with `#`, so neither the reading's count nor a reader of the release lines takes it for a release, and the test asserts its text. One thing to know when reading the file: the header's number is the wall clock (`date +%s%N`) and the gone and release lines' numbers are the monotonic clock, under the same `# number` shape. A word in the docstring would do; nothing depends on it today.

### The fixes, planted back

| Fix | Plants | Result |
|---|---|---|
| D2: a cut gone before its release | a refused release always counts as gone; always ends the releaser; the gone line with no `#`; not counted; the step says "released" | each KILLED |
| G1: a refused `held` | taken as nothing held | KILLED by two tests |
| D1, up front | no check for `--hold-cuts`; no check for `+hooks`; `--hold-cuts` matched as part of a word | each KILLED by `test_skz_store.sh` |
| D1, the row | the count not a difference; ended in the window read as running; no line read as running; no word in the serve arguments; no reason for the verdict | each KILLED |
| D1, the verdict | a row whose releaser ended gets a verdict | KILLED |

Three plants pass:

- **Ended before the window read as running** (only the second reading looked at): equivalent, since a releaser is not started again, so one that was not running before is not running after.
- **The reading counts every line**, and **the reading says "running" whatever became of the releaser**: the shell line in `snapshot` has no test. It is right, by my run above. A test like that run (the line taken from the script, a file and a child process) would hold it; small, and I would not wait for it.

### The rest

- **The poll interval not taken:** accepted. The description now names the polls as a part of the gap to the model that is not separated, which is what mattered.
- **The description's figures** now give the window's releases beside the whole life.
- **CI on 775814c:** `lint`, which runs every changed test and shellcheck, is green; `test`, `unit-arm64`, `coverage` and `s3-backends` were still running when I wrote this. The rigs were cancelled by testing, with the list on the pull request; none runs a changed file. Whether cancelled counts as green is the lead's.
- **Codex** has no open finding on 775814c.

Notes: `review/m2.t4/next_look.md`, `plants2.py`, `plants2.out`, `reading_line.sh`, `reading_line.out` on `skyzfs-review`.
