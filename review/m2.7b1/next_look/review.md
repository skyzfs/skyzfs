# Review of skyzfs/skyzfs#175 (M2.7b-1), the next look: at 895b18d

Head `895b18dd3b4d5c93839f3ea73b4158ffb3383535` (checked with
`gh api repos/skyzfs/skyzfs/pulls/175 --jq .head.sha` at the start and
at the end). Since the first look (at 5fc637b, `../review.md`):

- 2cc3471, the author's answer: the `refused store` text, the outcome
  cleared at a retire (D4), the pin note (D2), three tests (G1-G3), the
  `ended` text, and the nits;
- 5f89478, `zfs-topology` merged in at add63cd, which brings #176 (the
  publish rig speaks protocol 3 or 2) and #177 (the agent and cut rigs
  read protocol 3's `cuts`);
- 895b18d, the daemon's channel test's stub words a store refusal as
  the uploader now does.

I judged only #175's own delta (2cc3471 and 895b18d); the merged-in
content was reviewed on #176 and #177. I formed the view below from the
code first, and read the author's answer (issue comment 7210784305)
after.

Paths such as `uploader.rs` and `publish.rs` are under
`crates/skyzfs-upload/src/` unless another crate is named.

## Verdict

**Can merge.** Nothing blocks, and CI is green on the head: every job
of the ten runs on 895b18d ended `success`, or `skipped` where a job
does not apply (below).

- **B1 is settled.** Protocol 3 runs end to end on the combined head:
  the publish rig and the agent rig's `r` part, which failed at 5fc637b,
  are green at 895b18d, and so are the cut rig's three jobs (below).
- **The answer holds.** Each of the six plants the author names is
  killed by the test they name; L1's text, D4, G1, G2 and G3 are right
  in the code. The gates are clean, with the author's counts.
- What I found is small: two low defects in what D4 left behind (the
  protocol's documentation of `unknown`, and the agent's fake daemon,
  which still answers `done` after a retire), two low ones on texts
  (the `ended` of a retire, and a conflict's wording in a case not
  expected), four test gaps with scratch tests that close them, and
  nits.

## Defects, most severe first

Numbered on from the first look's (L1, G1-G5, D1-D4, N1-N6).

### L2. The protocol's documentation of `unknown` does not say that a retire ends it (low; certain)

`crates/skyzfs-control/src/lib.rs:111-112`: "`unknown`: not asked of
this daemon's uploader, or its end has left the 1024 kept." Since D4, a
retire that lands or finds `snap/T` gone also makes a publish that read
`done commit=H` read `unknown` (`Publishing::retired`,
`publish.rs:724-734`, called from `Driver::retire`, `driver.rs:458-460`).
The `retire` paragraph (`lib.rs:135-151`) does not say it either, nor
does `Progress::Unknown` (`publish.rs:302`: "or its outcome has left the
view", which a reader would take for the 1024) or the help text
(`bins/skyzfsd/src/cli.rs:365`). These are the documents M2.7b-2's agent
is written against. A sentence in each of the two paragraphs closes it:
"or a retire of T has landed or found it gone since (M2.7b)".

The rule itself is sound, as far as I can find (the brief asked for each
of these):

- **The publish's state machine.** `retired` touches only the outcomes
  and their order, not what pends, the cuts in view or the ended list
  the request log reads. A retire of a name that pends is refused at the
  ask, and a publish accepted after the retire was queued and before its
  turn pends when the retire is made: `retired` leaves it, the retire is
  made first (retires come first in a turn), and the publish then looks
  `snap/T` up anew. A retire refused `other`, or failed, keeps the
  outcome; one asked again that finds `gone` clears it then.
- **A later publish of T.** `Publishing::answer` already dropped the
  outcome of a name asked again (F5 of #122); after a retire there is
  none to drop, and the publish is looked up and made as any.
- **`withdraw`.** Of a name that does not pend, it answers as `progress`
  does: `unknown` after a retire, as for a name never asked.
- **`look`.** Reads object storage, not the uploader: `absent` after
  the retire, as before.
- **Crash and restart.** The outcomes are in memory only, so a daemon
  restarted after a retire already answered `unknown`; D4 makes the
  daemon that made the retire answer the same. A retire that landed
  unheard (an error) keeps `done` until it is asked again and finds
  `gone`, which is what `refused store`'s advice (ask again) leads to.
- **The agent.** `plan.rs` reads `unknown` for a publish it follows as
  "asked anew" (`crates/skyzfs-agent/src/plan.rs:921-925`); the agent retires only what it has
  recorded as published and no longer follows, so it does not ask
  `progress` of a retired name. M2.7b-2 should not read `progress` to
  find the commit to retire; `look` is the call for that.

### L3. The agent's fake daemon answers a retire as the daemon did before D4 (low; certain)

`crates/skyzfs-agent/src/engine/fake.rs:1333-1342` ("As the daemon
retires (M2.7b-1)"), added by this pull request: at `retired` or `gone`
it removes `snap/T` from its refs but leaves `self.daemon.ended`, so
the fake's `progress` (`fake.rs:1346-1357`) still answers `done` after a
retire, where the daemon now answers `unknown`. It also answers a retire
of a name that pends (the daemon: `refused asked`) and a retire after
`stop_uploader` (the daemon: `refused stopped`). Nothing sends a retire
today, so nothing fails; M2.7b-2's agent tests will run against the
fake. The fix is a few lines: remove the name from `ended` at `Retired`
and `Gone`, and refuse `asked` and `stopped` as the daemon does. It can
be here or the first commit of M2.7b-2; the lead's call (D5).

### L4. A retire dropped at the uploader's end is told it "may still be made" (low; certain)

`bins/skyzfsd/src/control/handler.rs:246-266`. A request that the
uploader dropped unanswered now reads `refused ended the uploader has
ended; {again}`, with the same `again` as `no-answer`. For a retire
that is `RETIRE_AGAIN`: "the retire may still be made: ask again, which
answers gone once it is". That is `no-answer`'s advice (the uploader
takes it at its next turn); after `ended` there is no next turn, this
daemon answers `ended` to every request after, and only the next daemon
can say `gone`. `Code::Ended`'s own documentation
(`skyzfs-control/src/lib.rs:465-468`) says what fits: "whose
transaction may have landed: ask again". A text of its own for `ended`
(for example "the retire may have landed: ask the next daemon, which
answers gone if it did") closes it; the publish's `again` reads well
enough after `ended`. Only a person reads it; the agent reads the code.

Two smaller things about the same change:

- The author's answer quotes the new text as "the uploader has ended;
  the request may have landed: ask again". The code prints the
  publish's or the retire's `again` (above), not that.
- The answer says that no test reaches that path. The existing test
  `an_uploader_that_has_ended_reads_ended`
  (`bins/skyzfsd/src/control/tests.rs:1373`)
  reaches it already, with a stub that drops the request
  (`uploader_with(|control, _| drop(control))`), and checks only the
  code. Scratch test S4 checks the text the same way and kills the
  plant `ended-text` (below).

### L5. A retire refused for a missing lease row is worded as three moves (low; edge)

`uploader.rs:340-349`. `retire_failed` words every
`RemoteError::Conflict` as "object storage: snap/T moved under each of
3 tries". `Remote::retire` returns `Conflict` after the tries, but also
at the first try where the refs store finds no lease row for the volume
(`RefError::LeaseLost` with no holder maps to `Conflict`,
`crates/skynbd-engine/src/lib.rs:67`; `remote.rs:379`'s `?`). The daemon makes
the row when it takes the lease, so this is not expected; the text would
then send a person looking for a ref that moved. Not worth a change on
its own; worth knowing if the text is touched again.

## Test gaps (plants that survive)

Eighteen textual plants of the delta, one at a time, each restored
after (`plants.json`, `plants.out`). Seven remake the author's six (the
doubled prefix twice: in the channel and in the uploader); eleven are
mine, on `retire_failed`, `Publishing::retired`, `Driver::retire`'s
match and the handler's texts; a nineteenth, the same edit as
`conflict-text-noprefix` with more context, ran in the second scratch
pass (below). `mk.py` writes `plants.json` (each
plant's file, the text replaced and put in, and the command) and checks
that each text occurs once; `plant.py` plants them one at a time in a
worktree of the head; `runplants.sh` is the driver. Unplanted, `cargo
test -p skyzfs-upload` and `cargo test -p skyzfsd --features test-hooks
--lib` pass first. The upload crate's plants run its whole test suite,
the daemon's its library's tests.

| Plant | What it does | Outcome |
|---|---|---|
| `d4-kept` | the outcome kept after a retire | killed: `a_retire_ends_what_was_kept_of_the_publish` |
| `d4-at-other` | the outcome cleared at `other` too | killed: the same |
| `split-txn` | the ref and the row in two transactions | killed: `a_retire_that_lands_unheard_deleted_the_ref_and_its_row_together` |
| `store-as-stopped` | a store error answered `stopped` | killed: `a_retire_that_object_storage_fails_is_refused_store_and_can_be_asked_again` |
| `doubled-prefix-channel` | "object storage: " added again by the channel | killed: `each_request_goes_to_the_uploader_and_its_answer_back` |
| `doubled-prefix-uploader` | "object storage: " added again by the uploader | killed: the G2 test |
| `drop-at-make` | the retires that wait cleared when the driver is made | killed: `a_retire_asked_before_the_lease_is_held_is_answered_once_it_is` |
| `conflict-text-generic` | a retire's conflict worded as a publish's (L1's second half undone) | **survives** (G6); killed by S5 |
| `conflict-text-noprefix` | the conflict's text without "object storage" | **survives** (G6); killed by S5 (as `conflict-text-noprefix-src`, the same edit with context that S5 does not share) |
| `retired-noop` | `Publishing::retired` does nothing | killed: the D4 test |
| `retired-no-order` | the outcome removed, the name left in `order` | **survives** (G7); killed by S6 |
| `retired-pending-too` | a publish of the name that pends dropped too | survives (G9, not worth a test) |
| `retired-only` | cleared at `retired`, not at `gone` | killed: the D4 test |
| `gone-only` | cleared at `gone`, not at `retired` | killed: the D4 test |
| `cleared-on-error` | cleared also when the retire failed | **survives** (G8); killed by S7 |
| `ended-text` | a dropped request reads the bare "the uploader has ended" | **survives** (G10); killed by S4 |
| `retire-publish-advice` | a retire's waits given the publish's advice | survives (text only; S4 does not pin which advice) |
| `store-reply-bare` | the channel's "the retire may have landed: ask again" dropped | killed: `each_request_goes_to_the_uploader_and_its_answer_back` |

So the author's six are confirmed. The gaps, each small:

- **G6. L1's conflict text is not pinned.** No test reaches
  `retire_failed` with a `Conflict`: the test of the tries is at the
  `Remote`, below the uploader. S5, a unit test in `uploader.rs`, pins
  both arms of `retire_failed`.
- **G7. `order` is not pinned at a retire.** With the name left in
  `order`, a name retired, published again and ended is dropped from
  view early, by the stale entry, once 1023 others end after it. S6, a
  unit test in `publish/tests.rs`, shows it.
- **G8. A failed retire keeping the outcome is not pinned.** The D4
  test checks `other`, `retired` and `gone`, not an error; S7 (in
  `tests/uploader.rs`) checks that a store error leaves `done` and the
  retire asked again clears it.
- **G9.** A publish accepted between a retire's ask and its turn is
  the only way `retired-pending-too` differs, and it needs a retire and
  a publish of one name at once, which the agent does not do. I would
  not ask for a test; the doc comment of `Publishing::retired` ("the
  uploader refuses such a retire before it is made") could say "or it
  was asked after the retire, which goes first".
- **G10. The `ended` text has no test** (L4). S4, in
  `control/tests.rs`, checks that a dropped retire and a dropped publish
  each read `refused ended the uploader has ended; ` and then their
  verb's advice.

`runscratch.sh` appends S4-S7 at the head in the plant worktree (S4
and S5 inside the last module of their files, S6 and S7 at the end) and
runs rustfmt, clippy with `-D warnings`, the four tests and the plants
they are meant for; `scratch-tests.out` has the lines. Each scratch test
passes at the head and fails on its plant. The first pass's rustfmt
check failed on one line of S4, which `scratch-control.rs` now has as
rustfmt wants it, and its `conflict-text-noprefix` matched twice (S5
holds the same text); `runscratch2.sh` is the second pass, with rustfmt
clean on the four files and the plant given context.

| Test | File | Kills |
|---|---|---|
| S4 `review_s4_a_request_the_uploader_dropped_reads_ended_with_advice` | `bins/skyzfsd/src/control/tests.rs` (`mod uploader`) | `ended-text` |
| S5 `review_s5_a_retire_that_gave_up_is_worded_as_a_retire` | `uploader.rs` (`mod tests`) | `conflict-text-generic`, `conflict-text-noprefix-src` |
| S6 `review_s6_a_name_published_again_after_its_retire_stays_in_view` | `publish/tests.rs` | `retired-no-order` |
| S7 `review_s7_a_retire_that_failed_leaves_the_publish_done` | `tests/uploader.rs` | `cleared-on-error` |

The author may take them as they are; none is needed for the merge.

## Nits

- N7. `bins/skyzfsd/src/cli.rs:379` is 87 columns where its paragraph
  wraps at about 70 ("text names it), which is left as it is. A retire
  goes through the uploader, under").
- N8. `remote.rs:179-183` (D2's note) says "the lead's ruling"; the
  author's answer says "Pending the lead's ruling". One of them is
  wrong; if the lead has not ruled, the code comment should not say so.
- N9. The uploader's module docs (`uploader.rs:110-120`) say a retire
  "is refused at once where the asker has stopped waiting": at the ask
  it is dropped unanswered (nobody is there to hear), and one queued
  whose asker left is skipped at its turn. The answer says N6 (closed
  asks that wait while no lease is held) is documented there; it is
  only implied. Neither matters.
- The first look's N5 (the request log's `retire` line has no commit)
  is left for testing to ask for, which I agree with.

## Decisions for the lead

- **D2 (pins).** The note is in `Remote::retire`. Confirm the ruling,
  or have the comment say it is pending (N8).
- **D4.** The rule the author chose (cleared at `retired` and `gone`,
  kept at `other` and at an error) is sound; L2 asks only that the
  protocol's documentation say so, here.
- **D5 (new). The agent's fake (L3)**: fixed in this pull request, or as
  the first commit of M2.7b-2, before its tests lean on the fake's
  retire.

## B1 on the combined head

- `tests/kernel/publish_agent.py:103`: `HELLOS = ('skyzfs-control 3',
  'skyzfs-control 2')`; `parse_answer` takes `cuts` with or without
  `rewound=` (`:145-148`); `raw` offers 3 and falls back to 2 only
  where the daemon's greeting says 2.
- `tests/kernel/cut-record.sh:1652` (`past_top`): accepts `opens=itself`
  at the line's end or followed by ` rewound=`; `pchannel` learns the
  daemon's greeting (`:2022`).
- `tests/kernel/agent_check.py:659`: `CUTS_LINE` takes an optional
  ` rewound=R`.
- CI on 895b18d (read the colour and the job names, not the tables):
  "publish through skyzfsd and check, ubuntu-26.04" success (run
  45946798576: the job 7 minutes, the run 10, within the 7 to 16
  minutes of the workflow's other green runs today; at 5fc637b it
  failed in every course); "the agent through skyzfsd, r, ubuntu-26.04" success (run
  45946798583), and `a1`, `a2`, `d` success; the cut rig's three
  "record through skyzfsd and check" jobs (ubuntu-26.04,
  ubuntu-26.04-arm, ubuntu-26.04-upload) success (run 45946798488);
  the replay rig's "write into a hole" and "through skyzfsd" jobs,
  e2e, k3s, ci (lint, test, unit-arm64, s3-backends), skyzfs-ci
  success; the rest under "CI" at the end.
- Codex re-reviewed 895b18d with no findings; its one finding (the P2
  on `lib.rs:790`, B1) has the author's answer.

## Gates

`gates.sh` in a worktree of the head after `rustup update stable`
(rustc 1.100.0, 2026-11-09), the target cleaned between feature sets;
`gates.out` has the lines.

| Features | fmt | clippy `-D warnings` | `cargo test --workspace` | `cargo doc`, `-D warnings` |
|---|---|---|---|---|
| none | clean | clean | 2037 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | | clean | 2096 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | | clean | 2061 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | | clean | 2037 passed, 0 failed, 16 ignored | clean |

The counts are the author's. They ran beside the plants on four cores,
which only slowed them.

## CI

Read at 06:16 UTC, the colour and the job names only: every check run
on 895b18d is `completed`, and each is `success` or `skipped`.

- skyzfs-daemon-rig (run 45946798508): "ubuntu-24.04, daemon",
  "ubuntu-26.04, daemon", "ubuntu-26.04-arm, daemon", "the table and
  its judge (no kernel)": success.
- skyzfs-agent-rig (run 45946798583): `a1`, `a2`, `d`, `k`, `r` on
  ubuntu-26.04 and "the check's logic (no kernel)": success.
- skyzfs-publish-rig (run 45946798576): "publish through skyzfsd and
  check, ubuntu-26.04" and "the agent's and the check's logic (no
  kernel)": success.
- skyzfs-cut-daemon (run 45946798488): "record through skyzfsd and
  check" on ubuntu-26.04, ubuntu-26.04-arm and ubuntu-26.04-upload, and
  "the check's logic (no kernel)": success.
- skyzfs-replay-rig (run 45946798467): "through skyzfsd" and "write
  into a hole, through skyzfsd" on ubuntu-24.04 and ubuntu-26.04, and
  "log reader and planning (no kernel)": success; the reference jobs
  ("replay, ...") skipped, as on a pull request that touches only the
  daemon's code.
- ci (run 45946798514): lint, test, unit-arm64, s3-backends,
  chart-lint: success. coverage, e2e (e2e-tier-a), k3s-smoke: success.
  skyzfs-ci's push run (45946792319): gate success, its lint, test and
  unit-arm64 skipped, as on a branch with a pull request open.

I did not read the rigs' tables, since nothing failed; the lead reads
them before the merge.
