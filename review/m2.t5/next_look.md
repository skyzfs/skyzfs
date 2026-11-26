# Next look at #158 (M2.T5 stage 1, the agent rig) at c580c72

Pull request: skyzfs/skyzfs#158 (`skyzfs-testing` into `zfs-topology`).
**Head: c580c72** (checked with `gh api .../pulls/158 --jq .head.sha`).
Base 77da00c; `zfs-topology` has moved to aa7c9a1 since (notes only).
Previous review: `review.md` (at de5eac3 and 0604977), posted as
issuecomment-7182337144; the author's answer is issuecomment-7182453072.
Line numbers are at c580c72. Plant scripts, outputs, show-cases and stubs
of this look are `pr158/next_*`.

c580c72 changes the agent rig only (`agent-record.sh`, `agent_check.py`,
their tests, the workflow's paths). 0604977 (the ZFS CLI recorder for
M2.6c-5) stays as its own commit, by the lead's ruling; it is judged at
the end.

## Verdict

- **Not ready to merge.** One blocking finding, B2: **CI's lint is red on
  c580c72** (shellcheck on the new shell test). A few lines' fix.
- **The lead's condition on the rigs is not met.** The cut rig, the
  cut-daemon rig and the node B rig on c580c72 were cancelled by hand at
  04:15, 30 to 50 s after they started (so were the daemon, replay,
  kernel-probe and publish rigs). None of them has run on this head; they
  must run, and be green, on the final head (a new one after B2's fix).
- **D1, D2, D3 are fixed as claimed**, and G3 holds in the judge. A
  destroyed published snapshot now ends FAILED, exit 1, in the judge even
  beside a premise ERROR (show-cases 1 and 2). D4, G2, G4 and the nit
  are fixed; G1 is with the lead.
- New, not blocking: G5 (the settle's status is judged nowhere: a settle
  that panics passes when the retry publishes, show-case 3), D5 (the rest
  of D4's kind in the recorder), G6 (a RigError still stops the judging
  of what follows it), G7 (an abandoned T is read at node B once), and
  tests that do not hold the new shell code (G8).
- 0604977: no finding.

## Blocking

### B2. ci.yml's lint fails on c580c72: shellcheck on `test_agent_record.sh`

Certainty: high (the CI log; reproduced with shellcheck 0.9.0, which
names SC2329 by its older number SC2317).
File: `tests/kernel/test_agent_record.sh:98-106`, the test of
`published()` added for D1.

Run 45754022996 (`ci`, pull request), job `lint`, step `shellcheck`
(`git ls-files '*.sh' | xargs -r shellcheck`, every severity) ends 123:

- line 99, SC2030: `fails=0` is set in the `( ... )` subshell;
- lines 100 and 101, SC2329: `zfs()` and `rig_error()` are never invoked
  as far as shellcheck can see (they are, through the eval'd
  `published`);
- line 106, SC2031: `fails` read after it was modified in a subshell.

The author's local gate was `shellcheck -S warning`, which hides these
info-level lines; CI does not. The `cases()` block above it carries
`# shellcheck disable=SC2034,SC2329` (line 55) for the same pattern.
Every later step of `lint` (fmt, the clippy runs, cargo doc) was
skipped by the failure, so they have not run on this head either.

## Defects, not blocking

### D5. The rest of D4's kind: the recorder still reports verdicts on the agent and the daemon as its own error

Certainty: high (by reading). Not blocking, as D4 was not: the run is red
either way and the log says why. But exit 2 says "not a verdict", and a
`rig_error` in the recorder stops the course before the check runs, so
every verdict of the cases already recorded is lost with it (a
`published-gone.tsv` line included: D1's answer holds only when the course
gets to its check).

`tests/kernel/agent-record.sh`:

- 384 (`frozen_until_withdrawn`): "the agent did not withdraw in 120 s".
  The rig has destroyed T (destroy) or the publish is too old (tooold);
  withdrawing is the agent's job and the daemon's answer, the very thing
  the two scenarios test. Scenario: an agent that never notices the
  destroy ends exit 2, NOT CHECKED.
- 445 and 474 (carry hooks, late): "did not reach asked:after in 60 s"
  with the store frozen: the daemon must accept an ask with no store.
- 482 (late): "snap/T had not landed 120 s after the thaw": the daemon
  publishing T after the thaw is the daemon's.
- 174 (`await_agent`): the agent's publish not ended within the rig's
  bound (700 s, or 300 s after the thaw for destroy, tooold and both,
  against the agent's own `--limit-s 600`).
- 223 (`reserve`): the daemon not ending 0 on SIGTERM.
- 524 and 549: `acheck points` fails when the traced run reached no
  point (an agent that fails before its first point), so D4's fix
  (`pin_trace_first_run`) is reached only when the trace has at least
  one line.

Suggested shape, as for D4: write the fact into the case's files and let
the check judge it, or at least run the check on the cases recorded
before a `rig_error`.

## Test gaps

### G5. The settle's status is judged nowhere (medium)

Certainty: high (show-cases 3 and 4, exit 0). File: `agent_check.py`
(`settle.rc` is read only for the INFO line at 656); `agent-record.sh:288`.

A plain case killed at `asked:after:1`; the settle, "as the next start of
the agent does", panics (101) and prints nothing; node B has no T; the
retry publishes another name. The criterion holds by the retry, the
outcome is INFO (a plain case with a kill), the record is clean: **exit
0**. The same with the settle ending 7 (unsettled at its limit). This is
D2's kind for the cases with a kill: the recovery run is what the rig is
for, and its crash is not seen as long as a later run copes.

In run 45751597306 every settle ended 0 or 3 (80 cases); 3 only where T
was given up (destroy, tooold, gone). A pin "the settle ended 0, or 3
where T's outcome is `abandoned ...`" fits every case of that run.

### G6. G3 holds in the judge for the lines judged before an error; an error of the rig still stops what follows (low)

Certainty: high (show-case 6, exit 2).
`agent_check.py:96-100` says "a FAILED line is never hidden behind an
error of the rig". It is not hidden when it was judged first. A RigError
raised in one case (a file missing, a line of the agent's output that is
not one of its four, `read_outputs` at 268; node B's tool failing at 666)
ends the loop: every later case, every name's node B pins and the
controls are not judged. Show-case 6: the second case lacks `holds.txt`;
the `gone` case after it left a hold of the tag; exit 2, and
`pin_gone_clean` is never written (exit 1 with FAILED once the file is
there). `pin_published_snapshots_exist` also comes after the watcher's
RigError (513-514), so a watcher error hides it.

Either say so in the docstring, or turn a case's RigError into that case's
ERROR line and go on.

### G7. An abandoned T is read at node B once (low)

Certainty: medium (by reading; I have not seen the daemon do it).
`agent-record.sh:291`: node B's `snap-row` of T is read once, right after
the settle. In destroy and tooold the store thaws once the withdrawal is
answered; a daemon that published the withdrawn T at a cut that lands
after that one read is not seen anywhere: the case's outcome is the
agent's `abandoned T destroyed`, and nothing reads T at node B again
(the controls and node B's pins take `published.tsv` only). A last read
of every abandoned name at the end of the course (`agent_end`) would
close it.

### G8. The new shell code has no test (plants survive)

`test_agent_record.sh` tests the helpers and, now, `published()`. The
recorder's halves of D3, D4 and G2 can be undone with every local test
passing (set C): T10 (`adump` drops the status), T11 (stderr into the
dump), T12 (no `record-after` dump: the check then fails on a missing
file, exit 2), T20 (no first publish in sweep), T21 (the traced status a
rig error again), T30 (`scen_both` writes 137 whatever the daemon did),
T31 (the daemon not killed, 137 written). Both functions run with stubs
in a few lines (`pr158/next_stubs.sh`; both behave as the author says:
`adump` keeps 6 and the error, `scen_both` writes 137, 0 or 7 as the
daemon ended).

### G9. The judge's plants that survive on the fixes

Set B (37 plants on c580c72's judge changes, 25 killed):

- N18 (`FIRST_RC['tooold']` set to 0): no tooold case with no kill in the
  tests.
- N20, N23 (`FIRST_RC['gone']`, `FIRST_RC['both']` changed): these two
  entries are never read, since both scenarios always kill at reach 1
  (`not_reached` needs N > 1). They are not from run 45751597306 either
  (its gone and both cases all ended 137).
- N62, N63, N64 (a missed control does not set `any_failed`): no test has
  a missed control beside an ERROR. The code ends 1 there (show-case 5);
  only the test is missing. G3's answer names both halves.
- Benign: N03 (a short `published-gone.tsv` line accepted), N04 (the file
  optional), N05 (the criterion on a retry whose name is gone: the gone
  pin fails anyway), N07 (only the last gone line kept: the pin still
  fails), N34 and N41 (`record-after.rc` and `first.rc` optional).

## The author's asks

1. **D1** (`agent-record.sh:251-268`, `agent_check.py:525-533, 624, 634`).
   `published()` lists the pool's snapshots and writes a name node A lacks
   to `published-gone.tsv`; the check fails
   `pin_published_snapshots_exist` and counts the name as published, so a
   destroy or gone case also fails `pin_ID_outcome` ("published, not
   abandoned destroyed"). Show-case 1: a destroy case whose T is gone and
   published ends 1 with both lines FAILED; show-case 2: the same beside
   another case's premise ERROR still ends 1. **Fixed.** No false FAILED
   in a right run: the rig destroys only the case's own T, with the store
   frozen (destroy) or before any ask (gone), so no right publish can
   name it; runs 45754023195 (a1, a2, d) wrote empty files. The new path
   hides no verdict: a gone name leaves node B's pins and the controls,
   but the run fails by the gone pin. What remains: exit 2 when the course
   stops on a `rig_error` before its check (D5), or the judge on an
   earlier RigError (G6); a pass when the publish lands after the one read
   (G7). Plants: N01, N02, N06 killed; shell T01, T03, T04, T05 killed.
2. **FIRST_RC** (`agent_check.py:154-166`, pin at 565-568). Read from run
   45751597306's four `cases.tsv` (artifacts of a1, a2, d, k): with no
   kill, plain 0 (4 cases), restart 0 (2), assist 0, destroy 3, tooold 3,
   sweep 0, unmarked 6, dhook 0 (4), late 0; every case with a kill
   ended 137. Each matches the program's codes
   (`crates/skyzfs-agent/src/program.rs:118-142`: 0 published, 3 given
   up, 4 stopped, 7 unsettled, 1 error). The comparison is exact, so no
   wrong exit passes, and none is too loose: 3 is "given up" for any
   reason, and `pin_ID_outcome` names the reason. gone and both are never
   read (G9). The pin applies to a case with no kill and to a later reach
   not made; unmarked keeps its own pin for 6. **Fixed.**
3. **G3**, end to end. The judge: `holds` and each control set
   `any_failed`, and `rc = 1` at the end (774-775) overrides 2 from a
   premise or a RigError. `cut-record.sh:585-586` records "checked exit
   1" and the job ends with the worst recording's status (2015-2016); one
   course per job, so the job ends 1. `cut_summary.py` prints the table
   (FAILED and ERROR lines both) and no exit, as before. **Holds** for the
   lines judged; not for what an error stops before it is judged (G6, D5).
   Plants N60, N61, N65 killed; N62-N64 survive (G9).
4. **D3, D4, G2, G4, the nit.**
   - D3: `adump` keeps the status and stderr (`agent-record.sh:280-284`,
     used for `record-before`, `record-after`, `record-stopped`);
     `clean()` fails a status other than 0 (315-317), unmarked needs
     `record-before.rc` 0 (604). Fixed. Nit: `record-stopped.rc` is not
     read (585), so a dump that fails while the agent is stopped reads as
     "had T at nothing, not asked", a premise ERROR.
   - D4: fixed at the two sites named (`pin_trace_first_run` through the
     first-run pin, `pin_ID_first_publish` at 607-612). The rest of the
     kind: D5.
   - G2: `scen_both` keeps the daemon's status (489-501) and
     `pin_ID_daemon_killed` needs 137 (613-619). Fixed. A FAILED (server)
     and not a premise is right: a daemon that ended by itself is the
     daemon's. Run 45754023195 d: 137 in all three both cases.
   - G4: the paths now hold all 18 crates `skyzfsd`, `skyzfs-agent-cli`
     and `skyzfs-nodeb` are built from (I computed the closure of path
     dependencies with `cargo metadata`: the same 18), and the Python the
     course runs. Missing: `tests/kernel/logwrites.py`, which
     `cut_check.py:82` imports at load and `agent_check.py` imports
     through it (nit).
   - Nit: the comments say `docker stop` / `docker start` (54, 113-114).
     Fixed.
   - **Plants** (one at a time, the file restored after each, 600 s
     limit; KILLED means the local tests failed):

     | Set | File | Plants | KILLED | SURVIVED |
     |---|---|---|---|---|
     | A: last look's survivors, redone | `agent_check.py` | 26 | 3 | 23 |
     | B: on c580c72's fixes | `agent_check.py` | 37 | 25 | 12 |
     | C: last look's shell survivors, and new | `agent-record.sh` | 26 | 4 | 22 |

     Set A: P13, P14 and Q01 (restart's, dhook's and both's EXPECTED)
     are killed now. Still surviving as last time: P02, P17, P18, P20,
     P43, P46, P48, P52, P58, P60, P61, P64, P65, P67, P73, P74, Q06,
     Q17, Q23 (a premise ERROR that skips the rest of its own case: the
     new G3 test has the ERROR and the FAILED in two different cases),
     R03, R06, R09, R11. Set B: G9. Set C: the twelve old shell
     survivors and T02, T06, T07 (matching rules of the gone path:
     benign with the names the agent makes), T10-T31 (G8).
   - **A run that should fail and passes**: show-cases 3 and 4 (G5).
5. **0604977** (`zfs-cli-record.sh` +89, `test_zfs_cli_record.sh` +32),
   as its own change. `record_reads` makes `skzrd` on a file and records
   the three reads the adapter runs (the list, and `zfs get -s local`
   with names and with values) in five states (tree, not, odd, counts,
   none) and the single-value gets; the cleanup trap now destroys `skzrd`.
   I read both artifacts of its dispatch run 45751625028 (24.04 and
   26.04): every read record is there, every status is 0 except the four
   of `none` and `value-missing` (1, "dataset does not exist"), and
   `about.txt` has no setup line about `skzrd`. The counts state shows
   `filesystem_count` and `snapshot_count` as local on `skzrd/child`
   (#157's blocking finding, recorded). No finding. Two remarks: a failed
   `zfs create -u P/odd-K` is not said in `about.txt` as the other setup
   steps are (the set's own record shows it); and the counts are recorded
   after `@t` only, so a fixture of the counts before and after a take
   (snapshot_count 0 then 1) is not there, if coding needs it. Its tests
   pass locally and in ci's lint job.

## Nits

- `test_agent_record.sh:57`: the `cases()` stub still writes
  `trace/publish.rc`, which `rec_aplain` no longer reads.
- Codex's P2 thread on 0604977 (`agent-record.sh:280`, the same finding as
  D3) has no answer on its thread; c580c72 fixes it. Answer and resolve it
  before "ready to merge". Codex has no finding on c580c72.
- Last look's: `test_agent_check.py` and `test_agent_record.sh` still run
  only in this workflow's `unit` job (not in ci.yml's lint job); the push
  trigger's `tests/kernel/agent*` does not match `test_agent_check.py`.

## CI on c580c72

| Run | Workflow | Result |
|---|---|---|
| 45754023195 | agent rig (pull request) | a1, a2, d **success** (checked exit 0; FAILED 0, ERROR 0, controls caught); k: see below |
| 45754022996 | ci | **lint failure** (B2); test, unit-arm64, s3-backends, chart-lint success |
| 45754023047 | coverage | success |
| 45754023320 | e2e | success |
| 45754013848 | skyzfs-ci (push) | success (gate only) |
| 45754023012 | cut rig | **cancelled** by hand at 04:15:52-04:16:07 |
| 45754023024 | cut-daemon | **cancelled** by hand at 04:15:47-04:16:00 |
| 45754023044 | node B rig | **cancelled** by hand at 04:15:46-51 |
| 45754022983, 45754023022, 45754023003, 45754022991 | daemon, replay, kernel probe, publish rigs | cancelled by hand |
| 45754014015 | agent rig (push) | cancelled by hand |

The head's agent rig, a1, a2 and d: 19 + 19 + 10 cases; every new pin
ok (`pin_ID_first_run` on the 7 cases of d with no kill and the two
traces, `pin_ID_daemon_killed` 137 three times,
`pin_published_snapshots_exist` ok, every `record-after.rc` 0); the
carry hooks: carry-put killed at cut 16, row cut 18 (C 17, G 21);
carry-swap killed at cut 20, row cut 22 (C 21, G 25).

K_RESULT

## Local runs (c580c72)

`python3 -I tests/kernel/test_agent_check.py`: 47 tests OK.
`bash tests/kernel/test_agent_record.sh`, `test_cut_record.sh`,
`test_zfs_cli_record.sh`: all passed. `test_publish_check.py`,
`test_publish_check_main.py`, `test_cut_check.py`, `test_nodeb_check.py`,
`test_cut_daemon_check.py`: OK. shellcheck 0.9.0 at default severity:
B2's four lines (as SC2030, SC2317, SC2031), plus info lines in the base;
CI's newer shellcheck reports only B2's.
