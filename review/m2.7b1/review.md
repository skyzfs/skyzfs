# Review of skyzfs/skyzfs#175 (M2.7b-1: the daemon retires a published snapshot) at 5fc637b

Head `5fc637b986806d964d787d25522ce13796359cf6`, one commit on
`skyzfs-coding-m2.7b1`, base `zfs-topology`. I checked the head with
`gh api repos/skyzfs/skyzfs/pulls/175 --jq .head.sha` at the start and
at the end, and it did not move. The branch starts at 6868430;
`zfs-topology` is at 6b16935 now (#171, #172 and #174 merged since, and
the lead's notes). `git merge-tree` merges the head with 402d44d and
with 6b16935 without a conflict; the agent crate's tests pass on the
merge with 402d44d (259 passed), and #174 changes only the agent rig's
shell and Python files.

I formed the view below from the code, design §5.2 ("Retention is ref
deletion plus `zfs destroy` locally"), the M2.7 row of the notes' work
packages and the protocol's own documentation, before I read the
description. Then I read the description and checked its claims and its
"Not done" readings (below).

Paths such as `uploader.rs` and `remote.rs` are under
`crates/skyzfs-upload/src/` unless another crate is named.

## Verdict

**Not yet.** The product code is right, as far as I can find; what
blocks is that the merge would break the kernel rigs, and CI on this
head already shows it.

- **B1 blocks: the rigs' stand-in still speaks version 2.** Each side
  of the channel now refuses the other version, so the rig's change
  cannot land before this pull request or after it: it has to be in the
  same merge. How that is done is the lead's decision (D1). The author
  names the publish rig and the cut rig. On this head the publish rig
  fails in every course, the cut rig's jobs pass, and the agent rig's
  `r` part fails too, which the description does not name.
- Nothing else blocks. One low wording defect (L1), three small test
  gaps with scratch tests that close them (the ref and the row in one
  transaction; a store error answered `store`; a retire asked before
  the first take), four decisions for the lead (D1-D4), and nits.

The gates I ran at the head are clean, with the description's counts
("Gates"). Of 32 plants in the new logic, 26 are killed by the pull
request's tests, 1 is equivalent, and 5 survive ("Plants"); the three
that matter are killed by the scratch tests S1, S2 and S3, which pass at
the head.

## Defects, most severe first

### B1. The rigs' stand-in speaks version 2, so the publish rig and the agent rig's `r` part cannot run (blocks; certain)

**Where.** Not in the diff: `tests/kernel/publish_agent.py`, which the
publish rig's courses (in `cut-record.sh`, `pchannel` among them) and
the agent rig both use, and `tests/kernel/cut-record.sh`.

**What fails, and how.**

- `publish_agent.parse_answer` accepts a `cuts` answer only with exactly
  the keys `attachment state count group opens`
  (`publish_agent.py:121`). The daemon's `cuts` now ends with
  `rewound=R`, so every `pagent ask cuts` raises `RigError` (exit 2).
- `HELLO = 'skyzfs-control 2'` (`publish_agent.py:87`) and the raw
  requests of `pchannel` (`cut-record.sh:1996` and `2022-2024`) greet
  with version 2. The daemon now answers each `refused version`, and
  `answer_of` reads no line after a greeting that is not its `HELLO`.
- `past_top` (`cut-record.sh:1651`) waits for a `cuts` line that *ends*
  with ` opens=itself`. After `rewound=R` is added it never does, so the
  wait runs out and the course ends in a rig error even once
  `parse_answer` reads the line.
- `agent_check.py`'s `CUTS_LINE` tolerates the new key (`\b.*`), so the
  agent rig's judge is not affected.

**Shown on this head.** "the agent through skyzfsd, r, ubuntu-26.04"
failed with exit 2 (run 45932127726, job 140286925138): `the rig's own
error in arun: the daemon's cuts after the export: rig: not an answer
line: 'cuts attachment=... count=62 group=95 opens=itself rewound=0'`.
That case (R-rewind, `agent-record.sh:877` and `882` on 6b16935) came
with #171, which merged after this branch's base, so the author could
not have seen it on 6868430; the pull request's CI runs the merge with
the base.

"publish through skyzfsd and check, ubuntu-26.04" (run 45932127781, job
140286936428) failed with exit 2 ("done: worst exit 2"), and no course
ran as meant:

- `pplain`, `prewindf`, `prewindc`, `pkill`, `passist`: a rig error at
  the agent's first `cuts` ("not an answer line: 'cuts ... opens=itself
  rewound=0'"); `prewind` the same at its first `cuts`.
- `pkillman`, `pkillpub`: "the daemon had not killed itself in its
  first publish 600 s on": no publish was asked, so the kill point was
  never reached, and each course spent its ten minutes waiting.
- `pchannel`: five pins read `FAILED: [server]` (`pin_answer_version`,
  `_malformed`, `_name`, `_peer`, `_cuts`, each answered `refused
  version` or, for the peer, read as no answer line), and
  `pin_status_control_malformed` reads 4, not 2. These are the rig's
  version-2 greetings, not the server; the summary's `[server]` tag is
  misleading here, which is one more reason not to merge on this run.

The rest of CI on this head is green as far as it has run: the cut
rig's three jobs (on uploads among them), the agent rig's `a1`, `a2`,
`d` and `k`, the replay rig's daemon jobs, the daemon rig on
`ubuntu-24.04` and `ubuntu-26.04` (its `ubuntu-26.04-arm` job was still
running when I last looked; I read the colour of these, not their
tables), unit, lint, coverage and `unit-arm64`.

**Why it blocks.** CLAUDE.md asks for CI green on the head before a
merge, and a merge as it stands would leave the publish rig and the
agent rig's `r` part red on `zfs-topology` until the rig changes. A
stand-in moved to version 3 first would break against the version-2
daemon on `zfs-topology` in the same way. Codex found the same thing
(its P2 on `lib.rs:789`, with `past_top`).

**What closes it.** The four points above, and the fixtures that greet
with version 2 in `test_publish_agent.py`, in the same merge as this
change (D1).

### L1. The text of `refused store` doubles its prefix and, after the tries, names the wrong cause (low; certain)

`uploader.rs:607-610` gives `RetireRefused::Store(e.to_string())`, and
`control.rs:130-133` (`bins/skyzfsd`) writes `object storage: {e}; the
retire may have landed: ask again`.

- For `RemoteError::Store(s)`, whose own text is `object storage: {s}`,
  the line reads `refused store object storage: object storage: s; ...`.
- After `RETIRE_TRIES` conflicts, `RemoteError::Conflict`'s text is
  "the commit was refused: head or the cut's row moved, or the volume
  has no lease row". For a retire it was `snap/T` that moved at each
  try; head and the row are not touched.

Only a person reads the text; the agent reads the code. A fix is a
variant of `RetireRefused` for the conflict, or a text built in
`turn_once` that names the ref.

## Test gaps (plants that survive)

The plants are textual, one at a time, each restored after; the command
for each is in `plants.json`, the outcomes in `plants.out`. Nothing
here is a defect in the code.

32 plants: 10 in `Remote::retire`'s CAS and tries, 11 in the
uploader's retire (the queue, the refusals at the ask, the stop, the
fence, the store, the lease beside it, a retire asked before the first
take), 4 in the protocol (version 2 at each side, `rewound` optional or
not written), and 7 in the daemon (the kill point, the log line, the
handler, the reply, the request's name). 26 are killed by the pull
request's tests. The ones that survive:

**G1. The ref and the row deleted in one transaction is not pinned
(`split-txn` survives).** A retire made as two transactions (the ref,
then the row if the first landed) passes all eight tests of
`tests/retire.rs` and the uploader's. No test makes the transaction
land and its answer fail. Scratch test S1 does that with the fixture's
`arm_after(FaultOp::Put, "refs", 0)`: the first answer is an error, the
ref and the row are gone together, and the retire asked again is
`gone`. S1 passes at the head and fails on `split-txn` ("Scratch
tests").

**G2. An error of object storage answered `store` is not pinned at the
uploader (`store-as-stopped` survives).** The daemon's test checks the
mapping from `RetireRefused::Store` to the reply, but no uploader test
makes a retire fail on the store, so an uploader that answered such an
error `stopped` passes. That matters to the agent: `stopped` says this
daemon will retire nothing more, `store` says ask again. S2 makes the
refs object fail for one retire: the answer is `store`, nothing is
deleted, and the next retire is made. It passes at the head and fails
on `store-as-stopped`.

**G3. A retire asked before the lease is first held is not followed
to its answer (`drop-at-make` survives).** `Control::Retire` says one
asked before the lease is first held waits for it. The pull request's
tests ask retires before the first turn only where another node holds
the lease, so they end `stopped`. An uploader that dropped the queue
when it makes its driver (the asker would read `ended`) passes. S3 asks
a retire while object storage fails every take, turns three times with
no answer and the state `NoLease`, lets the store recover, and expects
`gone`; it passes at the head and fails on `drop-at-make` ("dropped
unanswered").

**G4 (minor). `RETIRE_TRIES`'s value is not pinned (`tries-const2`
survives).** The test that gives up after the tries is written against
the constant, so 2 or 5 pass as well as 3. Pinning it is a one-line
assertion, if the lead wants 3 to be part of the contract.

**G5 (minor). The retire's step without the lease kept beside it
(`no-keeping` survives).** A retire is one read and one transaction, far
inside two thirds of a lease, so this matters only under a store that
hangs; I would not ask for a test.

**Equivalent: `stop-only-lease`.** Answering the retires that wait only
when the lease is lost, and not when the driver stops on a kept cut,
passes the tests; I believe it is equivalent. A retire is taken first in
the turn after it is asked, and alone, so retires can only pile up while
there is no driver (no lease yet). A kept cut needs a driver, and a
retire asked after it is refused at once by `control`.

## Decisions for the lead

**D1. How the stand-in's move to version 3 lands with this pull
request (blocks; B1).** Since neither version can serve the other, the
change to `publish_agent.py`, `cut-record.sh` and their tests has to be
in the merge that brings the daemon's version 3. Possible ways: the
testing session's commit is taken onto this branch (or merged into it
from a branch off it) before the merge; or the coding session makes the
change here, since it is mechanical, with testing reviewing it. The
author asks the same question.

**D2. Pins (not urgent).** `Remote::retire` deletes `snap/T` whatever
pins it has. `Engine::snapshot_delete`, the old path, refuses a pinned
ref (`skynbd-engine/src/lib.rs:983`). Nothing in skyzfs pins a ref
today, and design §5.4's clone makes a head ref of its own, not a pin,
so nothing breaks now. If M3 or M4 makes a clone or a browse pin
`snap/T`, the retire needs a check (and, in the transaction, something
that a pin added between the read and the transaction would fail).
Worth a line in the notes.

**D3. A status counter for retires (the author's reading).** I agree
it can wait: the request log's line serves the rig. M4's seven-day soak
("no leak in the store") will want one; M2.7b-2 or M4.

**D4. `progress name=T` after a retire still answers `done commit=H`.**
The uploader's memory of the publish is not touched by the retire
(`Publishing::progress`), so until the outcome is trimmed the daemon
says `done` for a snapshot it has just retired. Asked again, a publish
of T is looked up anew (F5 of #122's review), so nothing wrong is
published. M2.7b-2's agent must not read `done` as "still listed".
Either document it on `retire` or clear the outcome at the retire;
the lead's call.

## The author's "Not done" readings

- **The rigs speak version 2.** Agreed that it is testing's code, but
  it blocks the merge (B1, D1). The agent rig's `r` part and `past_top`
  belong on the list too; the cut rig's jobs pass on this head.
- **A course in the publish rig for `retired`, `gone`, `other`, a kill
  at `retire:N` and node B no longer listing.** Agreed, as a testing
  package after the merge. One thing for it: the request log's `retire`
  line carries no commit (`record.rs:291`, `-` in both sector columns),
  so a judge cannot tell from the log which commit a retire named or
  check `other` against it. Putting `H` in the line now would save a
  change of the log's format later (nit N5).
- **`refused other` to a publish stops the agent for a person.** Agreed:
  a daemon that answers it to a publish is not one the agent
  understands, and the other unknown refusals stop the same way.
- **A retire waits while the uploader holds no lease.** Agreed. One
  small cost: each `no-answer` leaves its closed request in `retires`
  until the driver is made (`control` checks `is_closed` only when the
  request arrives). That is one entry per ask while no lease is held,
  so the growth is tiny; nit N6.
- **A run that stops drops a retire under way; the asker sees `ended`.**
  Agreed, and `Code::Ended`'s documentation says the transaction may
  have landed. The handler's text for `ended` ("the uploader has ended")
  does not say so for a retire; a person reading the CLI's line is not
  told to ask again (nit, as for a publish before).
- **A row with no ref is left.** Agreed. Nothing lists rows (no
  `meta_list` of snapshot rows anywhere in skyzfs), `look` answers
  `absent` for it, and a later publish of T overwrites it (`MetaPut`),
  so it blocks nothing. Garbage collection's.
- **No status counter.** D3.

## What I checked, and how

**The CAS (`remote.rs:353-378`).** The retire reads `snap/T`; no ref is
`gone`; a ref of another commit is `other` with that commit; else one
transaction `[CasRef{expected: Some(read value), new_commit: None},
MetaDelete{row}]`. `ObjectRefStore::apply` checks every CAS before it
applies any op, and `RefValue` equality is commit and generation, so a
rewrite on the same commit (generation + 1) conflicts and is read
again; a conflict's `actual` is op 0's, the ref. Races on the refs
object itself (an etag) are retried inside `update_inner` and are not a
conflict, so the three tries count only moves of `snap/T`. The
transaction goes through the uploader's remote, whose refs are
`ActingRefStore` (`bins/skyzfsd/src/upload.rs:907`): `transact` is
`transact_as(holder, epoch)`, so the fence holds for the retire as for
a publish. A landed transaction whose answer is lost and is retried by
the store layer finds the ref gone and answers `gone`. Head and the
cut's row are not in the transaction, so a cut's commit racing it only
retries on the etag.

**The uploader (`uploader.rs:472-503`, `531-611`).** A retire is
refused at once when the asker is gone, when the uploader is stopped,
and when a publish of the name pends (withdrawn but not yet looked up
counts as pending, as it should). Otherwise it is queued and the next
turn begins at once. It is taken after the lease, the driver's making
and the stopped check, before the publishes and the cut, alone, under
`keeping`. A fence answers `stopped` and stops the uploader as a fenced
step does; any other error answers `store`; the turn's wait is zero.
`turn` answers every queued retire `stopped` after a turn that leaves
the uploader stopped. At `run_with`'s stop the turn and the queue are
dropped and the handler answers `ended`. A retire cannot hold the cut
up for longer than one transaction a request, and requests come one at
a time through the channel.

**Protocol version 3 (`skyzfs-control`).** `HELLO` is version 3; the
daemon refuses any other greeting (`control.rs:207`) and the client any
other greeting from the daemon (`client.rs:56`), each naming the
deploy order. `retire name=T commit=H` parses with both keys required,
H 64 hex digits; `retired` and `gone` take no keys; `rewound` is
required in `cuts` (`take` fails on a missing key), so a version-2
`cuts` line is malformed. The mark is the store's
(`skyzfs_local::inplace::Cuts::rewound`) read at the desk as the count
is, and its documentation matches the store's.

**The kill point and the log line.** `KillRemote::retire` counts only
`Retired` and dies after the inner transaction returned; the test sees
the refs object's `put_if` before the kill. The log line is written
after the uploader's answer, on the channel's thread.

**The agent.** `retired` and `gone` are not answers about a publish
(`Wait`); `refused other` ends a publish for a person; the fake answers
a retire as the daemon does. The agent does not send `retire` yet.

**Codex** posted one finding (P2, the rigs; B1 here).

## Gates

`gates.sh` ran them in a worktree of the head, after `rustup update
stable` (rustc 1.100.0, 2026-11-09), with the target cleaned between
feature sets; `gates.out` has the lines.

| Features | fmt | clippy `-D warnings` | `cargo test --workspace` | `cargo doc`, `-D warnings` |
|---|---|---|---|---|
| none | clean | clean | 2019 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | | clean | 2078 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | | clean | 2043 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | | clean | 2019 passed, 0 failed, 16 ignored | clean |

The counts are the description's.

## Plants

`plants.json` holds each plant (the file, the text replaced, the text
put in, the command), `plant.py` plants them one at a time in a
worktree of the head and restores the file after each, and `plants.out`
has every outcome with the tests that failed. `runplants.sh` is the
driver (with `SCRATCH` set to a scratch directory holding worktrees of
the head at `r175/head` and `r175/plant`). Before the plants, each
command passes unplanted: `skyzfs-upload --test retire --test uploader`
(8 and 65 tests), `skyzfs-control`, and `skyzfsd --features test-hooks
--lib` (202 tests).

One correction made on the way: the first run gave the daemon's plants
`--bin skyzfsd`, which holds no tests (the daemon's tests are in its
library), so all seven "survived" on zero tests. They were run again
with `--lib`, and all seven are killed; `plants.out` has the second run.

| Area | Plants | Killed by the pull request's tests | Survive |
|---|---|---|---|
| `Remote::retire` | 10 | 8 | `split-txn` (G1), `tries-const2` (G4) |
| The uploader | 11 | 7 | `store-as-stopped` (G2), `drop-at-make` (G3), `no-keeping` (G5), `stop-only-lease` (equivalent) |
| The protocol | 4 | 4 | none |
| The daemon | 7 | 7 | none |
| All | 32 | 26 | 6 |

With S1-S3 added, `split-txn`, `store-as-stopped` and `drop-at-make`
are killed, each by the one scratch test meant for it; `tries-const2`,
`no-keeping` and `stop-only-lease` survive, as expected.

## Scratch tests

`scratch-retire.rs` (S1, for the end of
`crates/skyzfs-upload/tests/retire.rs`) and `scratch-uploader.rs` (S2,
S3, for the end of `crates/skyzfs-upload/tests/uploader.rs`) use only
the files' own helpers. Appended at the head they are clean under
`rustfmt --check` and `cargo clippy -p skyzfs-upload --tests -- -D
warnings`, and the two test files pass whole: 9 and 67 tests.

| Test | At the head | Kills |
|---|---|---|
| S1 `review_s1_a_landed_retire_whose_answer_was_lost_took_the_row_with_the_ref` | passes | `split-txn`: the row is left, `(None, Some(row))` |
| S2 `review_s2_a_retire_that_object_storage_fails_is_store_and_the_uploader_goes_on` | passes | `store-as-stopped`: `Err(Stopped)` |
| S3 `review_s3_a_retire_asked_before_the_first_take_is_made_after_it` | passes | `drop-at-make`: "dropped unanswered" |

The author may take them as they are or fold them into their own
tests.

## Nits

- N1. `bins/skyzfsd/src/control.rs:8`: "The protocol (version 2)"; the
  first paragraph lists the count, the publish and the look, not the
  retire.
- N2. `bins/skyzfsd/src/cli.rs:378` (HELP): "refused other COMMIT"; the
  line is `refused other snap/T names commit H, not the request's; it
  is left as it is`.
- N3. `uploader.rs`'s module documentation (the numbered list of a
  turn's work, and "The daemon's channel") does not mention the retire
  step, taken first and alone; only `Control::Retire` does.
- N4. `skyzfs-control/src/lib.rs:178-179`: "The other codes are a
  publish's refusals"; `other`, and `asked`, `store` and `stopped` as a
  retire's answers, are a retire's too.
- N5. The request log's `retire` line has no commit (above).
- N6. Closed retire requests wait in `retires` while no lease is held
  (above).
