# Review of skyzfs/skyzfs#178 (the publish rig's retire courses, M2.7b-1) at b7202e1

Head `b7202e181786dc989bef78feb2730c222aa47ba9`, checked with
`gh api repos/skyzfs/skyzfs/pulls/178 --jq .head.sha` at the start and
at the end. Base `zfs-topology` at 14ae5ae, which has #175 (the daemon's
`retire`, protocol 3) merged. One commit, no product code: the
publish rig's courses `pretire` and `pkillret` in `cut-record.sh`, their
judge in `publish_check.py`, `ask retire T H` in `publish_agent.py`, the
ASCII-digit nit of #176, the workflow's course list, and tests.

I formed the view below from the code (the daemon's retire as built:
`Remote::retire` in `crates/skyzfs-upload/src/remote.rs:170-184,359-385`,
the uploader's `Control::Retire` and its turn in `uploader.rs:304-355,
498-521, 610-637`, `Driver::retire` and `Publishing::retired` (D4),
`bins/skyzfsd/src/control.rs:112-137` and `control/handler.rs:194-207`,
the request log in `record.rs:128-141,286-352`, the hook in
`kill.rs:18-21,872-887`, and the protocol in
`crates/skyzfs-control/src/lib.rs:22-180`), the design (§5.2, §9's M2
row), the notes (M2.7b-1 as built; #175's review rows) and #175's review
notes, before I read the pull request's description.

## Verdict

**Not yet.** One thing blocks, and it is small:

- **B1.** A retire torn at the kill (the ref deleted and the row left,
  or the other way round) is what `pkillret`'s kill point exists to
  catch, and the check reports it as the rig's own error (exit 2), not
  a verdict (exit 1). Shown with the check itself (`showcase.out`), and
  a prototype of the fix passes the tests (`b1-prototype.diff`).

And one gap I would close in the same commit, the lead's call (D3):

- **T1.** Neither course asks `progress` after a retire, so D4 (the
  outcome is cleared: `unknown` after `retired` or `gone`, kept after
  `other`) is not shown on a kernel. Two `pask` lines and three table
  rows.

Everything else holds. `RETIRES` and `ABSENT` say what the daemon
promises for what the courses drive, the courses are wired into every
list that names publish courses, and 64 of 67 plants are caught (the
three that survive are nits). The courses ran green at this head on
26.04 (the publish rig's push run and the pull request's own run,
below). CI is not all green yet: one
failure, the known `runtwice` flake in `test_agent_record.sh`, which
this pull request does not touch; it needs a rerun.

## The two questions

### (1) Do `RETIRES` and `ABSENT` hold what the daemon's retire promises?

Item by item, against the daemon as built:

| Promise | Where the daemon makes it | Judged here | |
|---|---|---|---|
| `retired` where `snap/T` named H | `remote.rs:369-380`, `control.rs:115` | `pin_answer_e1-retire` | yes |
| `gone` with no `snap/T`: a retire that landed, asked again; a name never published | `remote.rs:363-365`, `control.rs:116` | `e1-again`, `e9`, and `k1-again` on the next instance | yes |
| `refused other`, naming the commit `snap/T` names, nothing deleted | `remote.rs:366-368`, `control.rs:117-123` | `pin_e1_other_names_its_own` (exactly one 64-hex word, `\b`-bounded, equal to e1's `done`), `pin_e1_kept` (a look reads `snap commit=H(e1)` after it) | yes |
| `refused asked` (a publish of the name pends) | `uploader.rs:510-512`, `control.rs:128-131` | not driven | T3 |
| `refused stopped`, `no-answer`, `ended` | `uploader.rs:506-508,526-529,629-632`, handler | not driven (no hook; the description says so) | acknowledged |
| The request log: `publish - retire - - T ANSWER`, the answer without a refusal's text, written after the uploader's answer, so none from an instance killed after the transaction | `record.rs:133-140,291` | `RETIRES`, exact per instance and in order: `pretire` [e1 refused other, e1 retired, e1 gone, e9 gone]; `pkillret` [[], [k1 gone]] | yes |
| `progress name=T` answers `unknown` after `retired` or `gone`; kept at `other` (D4) | `driver.rs:456-462`, `publish.rs:724-734` | not asked | **T1** |
| `snap/T` and its row gone from object storage | one transaction, `remote.rs:369-379` | `ABSENT` (node B `snap-row`: no ref and no row) for e1, e9, k1; `e1-look absent`; node B's listing has no `snap/e1` and still `snap/e2` on its commit (`pin_e1_not_listed`); e2 is read in full (`PUBLISHED`), so a retire touched only its own name | yes |
| The ref and the row go together | same | at the end of each course (`ABSENT` judges both; plant A4 caught); at the kill only as a premise | **B1** |
| The name can be published again after its retire | `Publishing::answer` drops an old outcome (F5 of #122); after D4 there is none | not driven | T2 |

So the tables are right for what they cover: every entry matches the
daemon's words and rules, and each is load-bearing (plants R1-R7, A1-A4,
E1-E10 caught). What they do not cover is D4, `asked` and a republish.

### (2) The premise of `pkillret`

**Real, yes.** `k1-again`'s `gone` and the killed instance's empty
retire lines rest on the transaction having landed before the kill. The
hook prints its line only after `Remote::retire` returned `Retired`, so
after the refs transaction committed (`kill.rs:877-886`, `remote.rs:380`), and
`premise_k1_killed_by_the_hook` reads that line and `killed_rc 137`. The
read between comes after `serve_end` and before `serve_start 2`
(`cut-record.sh:2001-2008`); nothing else writes `snap/k1` in between,
and the next instance renews the same holder's lease at once
(`bins/skyzfsd/src/upload.rs:43-44`: the lease is taken at the start,
and a start that found another holder's would be refused), so its `gone`
does not wait out the dead instance's lease.
In the push run at this head the killed retire took 113 ms from the ask
to the kill, node B read `ref none`, `snaprow none`, and the next
instance answered `gone` in 3 ms.

**Tight, too tight for a premise.** It demands both gone, so neither
torn state passes it (K1-K3 caught). But a torn state is a fault of the
server, not of the rig: the hook has said that the transaction landed,
and one transaction deletes both. With the check as it is (S1, S2 in
`showcase.out`):

```
S1 pkillret: torn at the kill, the row left (ref gone)
   exit 2
   pin_k1_absent: FAILED: [server] a row
   premise_k1_retired_before_the_kill: ERROR: [rig] snap/k1 none, row there after the kill
S2 pkillret: torn at the kill, the ref left (row gone)
   exit 2
   pin_answer_k1-again: FAILED: [server] answered 'retired', not gone
   premise_k1_retired_before_the_kill: ERROR: [rig] ...
```

The verdict lines are there, but the premise's `RigError` makes the
run's exit 2, "the rig's own error, not a verdict", and skips
`pin_retire_lines`. `pkillpub`, the publish's kill after its
transaction, judges the same kind of fault as a verdict (S3: exit 1,
`pin_p1_found`), because its premise asks only that something landed
(`premise_p1_landed`: the ref or the row). B1.

**Positive control.** Nothing in `pkillret` reads `snap/k1` before the
retire (`PUBLISHED['pkillret']` is empty), so "node B finds none" has no
read of its own that shows node B would find it. The hook's line implies
the ref was there (it fires only on `Retired`), so this is low (T4).

**Does the course show what a kill at `retire:N` should?** At `retire:1`
(after the transaction): both gone at the kill, `gone` from the next
instance, no retire line from the killed one, and `absent` at the end,
yes. Both there or both gone, never one: a "never one" failure is seen,
but as the rig's error (B1). The other side, a kill before the
transaction (both there, the next instance answers `retired`), has no
hook; the description says so.

## Defects, most severe first

### B1. A retire torn at the kill is the rig's error, not a verdict (blocks; certain)

`tests/kernel/publish_check.py:1295-1301`, and the test that pins it,
`tests/kernel/test_publish_check_main.py:846-850`
(`test_k1_there_after_the_kill_ends_2` asks exit 2 for both torn
states). Failing scenario: a daemon whose retire deletes `snap/k1` and
leaves its row (or the reverse) at `--kill-at-upload retire:1`. The
course sees it, and the run ends 2 with `rig_error` (S1, S2 above). Per
CLAUDE.md exit 2 is "an error of the rig and not a verdict on the
server"; this is the class of #141's B2 (a real fault reading as the
rig's error).

Fix, as `b1-prototype.diff` (tested: the unit tests pass, 67 with one
added, and S1, S2 then end 1): the premise is that the transaction
landed, as the hook says (the ref or the row is gone); a pin
`pin_k1_retired_whole` [server] asks that both are. "Both there" stays
the rig's error, as `pkillpub`'s "neither there" is (the hook said it
landed and nothing did). If the lead prefers, "both there" can be a
verdict too, since the hook is the daemon's.

### L1. CI: one failure, the known `runtwice` flake (low; certain it is not this pull request)

`ci.yml`'s `lint` job (run 45953135341, job 140353406428) failed at
`bash tests/kernel/test_agent_record.sh`:

```
FAIL runtwice: two signals of two kinds; the second ends it 1:
  got 'rc=1 rounds=none sig=SIGINT, ended=after verdicts='
  want 'rc=1 rounds=none sig=SIGINT,SIGTERM, ended=after verdicts='
```

The same failure as on #173 at 3b93a83 (`review/m2.7c/next_look`): the
stand-in agent's SIGINT handler runs nested in its SIGTERM handler. This
pull request touches neither `agent-record.sh` nor its test, the same
step passed at this head in `skyzfs-ci`'s push run 45953051678, and it
passed twice here. A rerun of the job is needed before "ready to merge";
the race itself is a small fix for testing (second sighting).

## Test gaps

- **T1. D4 is not shown on a kernel** (medium; certain by inspection:
  `rec_pretire` asks no `progress`, and `EXPECTED['pretire']` has no
  such label, so a daemon that answered `done` after a retire, D4
  reverted, passes both courses). In `rec_pretire`: `pask
  e1-kept-progress progress e1` after `e1-kept` (`done `, and a pin that
  it names e1's commit: `other` keeps the outcome), `pask e1-progress
  progress e1` after `e1-retire` (`unknown`), and `pask e2-progress
  progress e2` (`done ` on e2's commit: a retire clears only its own
  name). `RETIRES` is unchanged (`progress` writes no line).
- **T2. The republish of the same name** (low-medium). After e1's
  retire: `zfs destroy pool@e1` (the flow releases its hold), `pflow e1`
  again. It would show that the daemon makes `snap/e1` anew on a later
  cut, that `look` reads the new commit (not `unreadable other-commit`,
  the word kept for "a row of another commit, beside a ref deleted and
  made anew"), and node B reads it in full. e1 then moves from `ABSENT`
  to a read between (as `k1-between`).
- **T3. `refused asked`** (low): a retire of a name whose publish pends
  (a `pflow` with `--group-plus 1000000`, as t7, f1 and a2 are asked),
  then its withdrawal; `RETIRES` gets `refused asked`.
- **T4. No read of `snap/k1` before the retire in `pkillret`** (low): a
  `k1-before.txt` (ref on H, row there) would give the read between a
  positive control of its own, and show "both there" before "both gone".
- **Surviving plants** (all low): G5 and G6, snap's `manifest-len` and
  `group` read by `isdigit()` alone are not caught: the non-ASCII rows
  cover `rewound`, the cuts' `count` and `group`, and snap's `number`
  only (the description says the fix covers `manifest-len`, which it
  does; nothing tests it). C6, `pkillret`'s wait for the daemon to die
  (`cut-record.sh:2001-2002`) is outside `steps_of`'s pattern; without
  it, a retire answered `no-answer` before the kill would end at
  `serve_end 5`'s "the daemon's pid went away" text, which misleads.

## Nits

- n1. `publish_check.py:7-9`, the module's list of courses, lacks
  `pretire` and `pkillret` (the later paragraphs name them).
- n2. `publish_agent.py:51` and `:249` say `refused other COMMIT`; the
  daemon's line is `refused other snap/T names commit H, not the
  request's; it is left as it is`.
- n3. `publish_check.py:449`, `judge_listed` reads the generation with
  `f[2].isdigit()`, the pattern this pull request replaces in the agent
  (harmless here: no `int()` follows); `is_hex64` (`:461`) is
  `publish_agent.is_hex(text, 64)`, which the module imports already.
- n4. The title says M2.T5; the publish rig's packages have been M2.T7
  (#139, #141, #176), and M2.T5 is the agent rig's (#171, #174, #177).
  The lead's call.

## Decisions for the lead

- **D1. The request log's `retire` line with its commit.** Testing asks
  for it (the description's "Not done"); the first look at #175 left it
  to testing to ask (its N5). It is a change of `record.rs`'s line
  (`-` in the sector columns today), so b-2's first commit, with D5 and
  L2 of #175, would be the place. `RETIRES` would then pin that each
  line answers its own request.
- **D2. B1's rule.** A torn state at a kill point is a verdict (my
  reading); whether "both there although the hook said it landed" is
  the rig's error (as `pkillpub`'s "neither there") or a verdict too.
- **D3. T1 here or later.** I would put it in this pull request: it is
  the one behaviour #175 changed late (D4) and the course already
  holds a done publish and its retire.
- **D4. The package's id** (n4).

## CI at b7202e1

At the end of this review (check runs of the head):

- **The courses, at this head**: the publish rig's push run 45953051669
  (26.04, one run, one runner): all eleven courses `checked exit 0`;
  `pretire` 21 lines ok, `pkillret` 11 ok, the same counts as the
  description's run on 4fe20bd. Its artifact's request logs show the
  lines `RETIRES` pins: `e1 refused other`, `e1 retired`, `e1 gone`,
  `e9 gone`; and for `pkillret` none from pid 18671 (killed, rc 137,
  the hook's line in `serve.log`) and `k1 gone` from 18908 (taken over).
  The pull request's own run of the publish rig (45953135284, 26.04,
  another runner) ended green too: all eleven `checked exit 0`,
  `pretire` 19 ok and 2 INFO, `pkillret` 9 ok and 2 INFO, the same
  retire lines, node B's read between `ref none`, `snaprow none`. Two
  runs at this head, both on 26.04; 24.04 is not run on a pull request.
- Green: the publish rig (push and pull request), cut-daemon (push),
  node B rig (push and pull request), `skyzfs-ci` (push: lint with
  shellcheck, test, unit-arm64), `ci.yml`'s unit-arm64, s3-backends and
  chart-lint, coverage, e2e, the agent rig's a1, a2, d and r, the
  no-kernel jobs of each rig.
- Failed: `ci.yml` lint, L1 (flake).
- Running: the cut rig's jobs (push and pull request), the agent rig's
  k, `ci.yml`'s test.
- Cancelled within about a minute of the start: the daemon rig, the
  replay rig and the kernel probe. None of their scripts reads a file
  this pull request changes (only the cut, cut-daemon, node B, agent
  and publish workflows use `cut-record.sh` or the publish files), as
  on earlier testing pull requests.
- Codex: no findings at b7202e1.

## What I ran

- `gates.sh` / `gates.out`: the publish rig's unit job's four commands
  and `test_agent_record.sh` at the head, all pass (38, 48 and 66 Python
  tests; both shell suites). shellcheck here is 0.9.0 (older than CI's):
  head and base differ only in that version's SC2317 notes on the new
  functions; CI's shellcheck passed at the head (`skyzfs-ci` push run).
- `plants.py` / `plants.out`: 67 plants, each alone in a worktree at the
  head, with `git checkout` after each and `timeout 600` on each test
  command; the checkout was clean at the end. Groups: R (`RETIRES` and
  its judge), A (`ABSENT`), K (the kill premises), E (`EXPECTED`,
  `PUBLISHED`), P (the pins of `pretire`), F (the main test's fixtures
  changed to a daemon that misbehaves), G (`publish_agent.py`), C
  (`cut-record.sh`). 64 caught; G5, G6, C6 survive. K5, E1, E2, E4 and
  E9 are caught only by `test_publish_check.py`'s exact copy of the
  tables, not by a behaviour test (E1 widens `e1-other` to any
  refusal, which the main tests never feed it).
- `showcase.py` / `showcase.out`: S1-S3 above, through the main test's
  own `run_course`.
- `b1-prototype.diff`: B1's fix, with the unit tests passing.
- The push run's artifact was read as data in a fresh directory (the
  `pretire` and `pkillret` results, request logs, agent logs, node B's
  listing and read between).
