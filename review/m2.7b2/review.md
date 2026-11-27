# Review of skyzfs/skyzfs#179 (M2.7b-2), from adc391a to e6530f1

The head moved three times during the review:

| head | what it added |
|---|---|
| `adc391a1e7136c1fda4494934d1a69c2cbbc03da` | the first five commits |
| `d68471fd1d166a5f83dc157a8a14cb4b9ac0259b` | the lead's ruling on reading 3: names whose destroy fails are stuck |
| `5caf2efabfca508fa55e7b872066d5c849acd231` | the lead overruled the retries of every stuck name each round: at most 8 a round, in turn |
| `e6530f15228adb385683fe2fdae2856b04aeb7ec` | docs only: what a frequent restart does to the turn |

**The verdict is on e6530f1.** I checked the head with
`gh api repos/skyzfs/skyzfs/pulls/179 --jq .head.sha` at the start, at
each move, and at the end. This note judges `7cf6da6..e6530f1`, eight
commits, and says which heads have each finding.

- **The base.** It is `7cf6da6`. `zfs-topology` has since moved to
  `e77be51`: #178 (the publish rig's retire courses) merged, and it
  touches `tests/kernel/` only. `git merge-tree` of the head with
  `origin/zfs-topology` is clean. #178's check reads the `retire`
  line both with and without the commits that adc391a adds (d5e5420).
- **What I read first.** I formed the view below before reading the
  description, from:
  - design §5.2 ("After a rewind import") and §6;
  - the M2.7 rows of the notes;
  - the earlier reviews of this work: `review/m2.7b1/` and its
    `next_look/`, and `review/m2.7c/` and its `next_look/`.

  Then I checked the description's claims one by one.
- **Paths and lines.** Paths are under `crates/skyzfs-agent/src/`
  unless another crate is named. Line numbers are at e6530f1.

## Verdict on e6530f1

**Not yet.** Two findings block in my reading.

- **M1 blocks, unless the lead accepts it.** A few hours of give-ups
  fill ABANDONED_MAX (256). `Record::room` then refuses every begin
  until the oldest give-ups age out, which takes `--abandoned-keep-s`
  (24 h).
  - **The scenario.** On the fake, an uploader answers `stopped` for
    5 h. Once it is back, no publish begins for about 19 h.
  - **A regression.** The base dropped the oldest give-up past 64 and
    published at once.
  - **Not covered by the ruling.** The lead's ruling on reading 3 is
    about the names whose destroy fails. This bound is another case.
- **M2 blocks, unless the lead names it a follow-up.** Retention looks
  at neither the failure note nor an end between its steps.
  - **What is shown.** On the fake, a volume lost during retention is
    stepped on 240 s after the failure. M2.7's criterion is "within
    seconds".
  - **What is likely, not shown.** With destroys that hang on the lost
    device, the host's cap of four children left running would also
    refuse the step's `zpool reopen`.
  - **The other heads.** d68471f widened this, and 5caf2ef bounds it
    again: at most 8 steps and 8 retries a round.
  - **Codex** raised the same (P1, now at retain.rs:176). Its thread
    is not answered yet.
  - **The fix is small:** stop between names when a note came or an end
    was asked.
- **The stuck names, from d68471f to e6530f1, follow the lead's
  rulings.**
  - The questions asked of d68471f: (a) and (b) are fair, and the lead
    has since accepted both; (c) is settled by 5caf2ef; (d) is not a
    weakening.
  - The turn of 5caf2ef does not starve a name when names are
    forgotten or leave during a turn. Only a process restarted more
    often than every few rounds does, which the new docs sentence of
    e6530f1 says. The sentence is right in substance but not exact, a
    nit.
  - The in-process turn is acceptable.
- **The rest is small:**
  - low defects L2 to L5, and the readings in "Decisions for the lead";
  - test gaps: plants that survive, four of them in the stuck names'
    own code (M5, M7, P2 and Q8);
  - nits.
- **Gates and CI.** The gates I ran are clean at adc391a, d68471f and
  e6530f1 (5caf2ef's code is e6530f1's), and the test counts match the
  description's. CI: see "Gates and CI".

## What changed between the heads

### adc391a to d68471f: the names stuck

d68471f is one commit, in `crates/skyzfs-agent` only (seven files,
+445 −28).

- **`Record::stuck`** is new in v3 (record.rs:164-184): up to
  STUCK_MAX (64) entries of `Stuck { name, members, at_ms }`.
  - `check()` gets four rules: more than 64; a name stuck twice; stuck
    and kept elsewhere too; stuck with no members.
  - `names()` finds a stuck name.
  - `room()` is unchanged in code. Its docs now say that stuck names
    count for no room.
  - A v2 record reads with no stuck names (record.rs:427).
- **`stick()`** (retain.rs:319-346) moves a name in one save. It moves:
  - a published name at its first failed destroy or failed members
    read (retain.rs:286);
  - a give-up past its time likewise (retain.rs:438).

  Past 64, the oldest stuck name is forgotten. The line
  `forgotten <t> (destroy failed; destroy by hand)` is told before the
  save that drops it.
- **`retry_stuck()`** ran first in each round and retried every stuck
  name, outside STEPS_MAX.
- **The dump** prints `stuck <t> members=<n> at_ms=<ms>`
  (program.rs:1921-1928).
- **Tests.**
  - `a_destroy_that_fails_is_told_once_a_reason_and_tried_each_round`
    now expects the name to be stuck.
  - Five new tests of retention, the record's rules, and the dump's
    line.
- **Effect on the findings of adc391a.**
  - L1 is fixed (below).
  - M2 is widened.

### d68471f to 5caf2ef: eight a round, in turn

- **The retries.** `retry_stuck()` (retain.rs:291-317) sorts the stuck
  names by name and starts after `Retention::stuck_last`, the name
  last tried in this process. It wraps to the first and tries at most
  STUCK_STEPS (8).
- **The turn.** `stuck_last` is set before each try. It lives in the
  process: `Agent::restart` (engine.rs:752-765) keeps it, and a new
  process starts at the first name.
- **Tests.**
  - New: `the_names_stuck_are_tried_a_few_a_round_in_turn`.
  - `past_stuck_max_the_oldest_stuck_is_forgotten` no longer counts
    the retries.

### 5caf2ef to e6530f1: docs only

The docs at retain.rs:44-47 add: "So a process restarted more often
than every `STUCK_MAX / STUCK_STEPS` rounds retries only the first names
by name; this has no other effect." There is no change in code.

## Defects, most severe first

### M1. Hours of give-ups stop every publish for about a day (blocks unless the lead accepts it; certain on the fake; a regression from the base; every head)

**Where.**
- `record.rs:573-578` (`room`): a begin needs
  `abandoned + under_way + 1 <= ABANDONED_MAX`, where ABANDONED_MAX is
  256 (record.rs:102).
- `engine.rs:963`: `begin` returns `Full` when there is no room.
- `retain_given_up` (retain.rs:389): a give-up leaves only after
  `--abandoned-keep-s`, 24 h by default.

**Scenario.** This is scratch test T5 (`scratch-program-tests.rs`), in
the program's fake world. `run` uses the default `--every-s 60`, and
the fake daemon's uploader answers `stopped`.
1. Each round begins a publish, which is refused and given up.
2. From about round 257, the rounds begin nothing (`Full`).
3. The uploader comes back at 5 h. The hour after that has 60 rounds,
   with 0 begun and 0 published. Each round says "no room in the
   record: 8 publishes under way, or the names given up or published
   at their bound while retention catches up".
4. The first begin comes 1082 minutes after that hour (T5b). That is
   about 19 h with no publish while the uploader works.

T5 fails in the same way at d68471f, and the code it runs has not
changed since. At the base 7cf6da6 the same test publishes in each of
the 60 rounds, since the base dropped the oldest give-up past
ABANDONED_KEPT (64).

**Why.** The bound is smaller than the default keep lets in. One
give-up a round, at 60 rounds an hour, comes to 1440 give-ups in 24 h,
far above 256. "While retention catches up" is not what happens: no
give-up falls due before its 24 h, so retention has nothing it may do.

**Fix options, for the lead.**
1. When a begin lacks room only because of the give-ups, retire the
   oldest give-up early, as retention would after its time:
   - destroy its members where they are recorded, after the look for a
     name a person gave up;
   - forget it with a line where they are not.

   A give-up holds no ref. An operator can do much the same today with
   a short `--abandoned-keep-s`, but the default should not stop
   publishes. This is the principle of the lead's ruling on reading 3:
   what retention may free does not hold the room that publishes need.
2. Make ABANDONED_MAX at least the default keep times the round rate,
   or shorten the default keep.
3. Do not begin a publish while the uploader answers `stopped`. This
   removes this cause, but not the others.

I would take option 1.

### M2. Retention does not look at the note or at an end between its steps (blocks unless the lead names it a follow-up; the delay is certain on the fake, the host's cap is likely but not shown; every head)

**Where.**
- program.rs:1669 checks `go()` and `note_came()` only before
  retention starts.
- Nothing checks them again during retention: not `retain()` between
  names (retain.rs:171-184), not `retry_stuck()` between stuck names,
  and not `destroy_members` between members.
- host.rs:54 sets STUCK_MAX to 4, and the step's `zpool reopen`
  (zfs/cli.rs:1669-1671) goes through the same `exec`.

**What is shown.** Scratch test T2: a volume lost at the first of
eight unanswered retires is stepped on 240 s after the failure (`code
11; the reopen 240000 ms after the failure; asked 8`).
- Design §10 and M2.7's criterion ask for the step within seconds.
- #173's F1 was the same gap in the round. `settle_while` closes it
  for the settles.

**What is likely, from the code but not shown** (the fake ZFS does not
go through the host).
1. **A destroy waits past its limit.** A destroy is a sync task. On a
   pool whose device is gone, the sync does not complete, so the
   destroy waits past its limit (120 s plus 100 ms a dataset, at most
   20 min). It cannot be killed, so it is left running. CLAUDE.md says
   so: "A command on a suspended pool may never return and cannot be
   killed", and a writer on a pool whose device has no server "does not
   end when told to".
2. **In the steady state.** Once `--keep` is reached, one name is
   retired a round. A volume lost between the check at program.rs:1669
   and a destroy delays the step by one such limit. The base had that
   risk only for the command in flight; retention adds a destroy to
   every round.
3. **With a backlog or stuck names.**
   - A round now does up to 8 steps and 8 stuck retries (d68471f: up
     to 64 retries).
   - So four destroys can hang in one round: four limits, 8 min or more.
   - The host then refuses every command, the step's `zpool reopen`
     included.
   - Those four end only once the pool resumes, and only the reopen
     resumes it, so only a new agent process gets out.
   - Meanwhile the pool goes on taking writes. In the product layout,
     a local log device acknowledges them.

**The signal rule.** Retention does not start once an end was asked.
But once it has started, its steps go on after an end is asked, and the
exit waits for them: on a pool that holds its commands, up to four
limits.

**The lost step's quick retry** is not slowed by retention, which does
not run in a round with a note. The step itself comes late (T2).

**Fix.** Pass `retain` a check, as `settle_while` has one. Stop when a
note came or an end was asked:
- between names;
- between stuck retries;
- between members' destroys.

### L1. Names whose destroy fails take every step (adc391a only; fixed by d68471f)

At adc391a, eight names whose destroy failed for good took all 8 steps
of every round. The marks then filled to 64, and the agent went Full
in about 16 h (scratch test T4). From d68471f such names move to
`stuck`, and T4 passes.

### L2. Unanswered retires hold a round for up to eight answer limits (low; certain on the fake; every head)

**Where.** The loop at retain.rs:171-184, and `retire_ref` at
retain.rs:219-268. A retire that is not answered is told and skipped,
and the loop asks the next marked name. Each `no-answer` costs the
daemon's ANSWER_LIMIT (30 s).

**Scenario.** Scratch test T1: eight marked names, and a daemon whose
uploader is in a long step. Round 1 took 240 000 ms at
`--every-s 60` (`took_ms=240000 ... skipped=3`).

**Fix.** Stop retention's asks of the daemon at the first `no-answer`
(or any error) in a round. A round with nothing else slow then stays
within its tick.

### L3. A name stopped `other` has no way out (low; certain; every head)

**Where.** retain.rs:175, and the module docs at retain.rs:81-83
("until a person settles it").

**Scenario.** Scratch test T3:
- the name stays `retire=other` in the record;
- its `@t` stays on the pool;
- it is never asked again.

Nothing in `skyzfs-agent` lets a person settle it: no verb drops it or
asks again.

It also counts among the marks. 64 such names stop all marking
(`RETIRING_MAX`), after which `published` grows to PUBLISHED_MAX and
the agent is Full. That takes 64 refs moved by a person, so it is
unlikely.

The lead accepted reading 2. What is missing is how a person settles
it: a line of docs, or a verb. That is for the lead.

### L4. "The newest" kept is by order of completion (low; reading; every head)

**Where.** record.rs:549-566 (`leave_published` pushes when a publish
is done) and retain.rs:194-212 (`.take(n)` in vector order).

**Scenario.** Up to 8 publishes are under way. One that stalled and
then finished is pushed after newer groups, so the names kept are the
last to finish, not the newest groups. At `--keep 1`, the newest
snapshot can be retired while an older one stays.

Also, `set_keep` (retain.rs:148) allows 0 at the engine. The CLI refuses
0 (plant G15 is killed).

**Fix.** Sort by group when marking, or say "the last published" in the
docs and the help.

### L5. A take destroyed whole is given up as `left=unknown` (low; certain by reading; every head)

**Where.** engine.rs:1688-1696 (`failed()`). After `zfs destroy -r`
succeeds (`Why::NotWhole`), the give-up still records `Left::Unknown`.

**Effect.** 24 h later the name leaves with
`forgotten <t> (no member guids; destroy by hand)`, though nothing is
left to destroy.

**Fix.** Record `Left::Nothing` when the destroy succeeded. Also,
`ManifestTooLarge` (engine.rs:1612) has the members it just read and
could record them.

## The questions asked of d68471f

### (a) A name given up whose destroy fails is stuck too: fair (the lead has accepted it)

Such a name holds no ref, so the ruling's reason applies to it as much
as to a retired one. Left among the give-ups, it would hold
`room()`'s give-up bound, which M1 shows is the tighter of the two.

A small point: a name a person gave up is looked up (`looked_absent`)
only before its first destroy, and its retries do not look again. A
`snap/T` that lands more than 24 h after its give-up is too unlikely to
matter.

### (b) A name moves at its first failed destroy, and a failed read counts as one: fair, with a low cost (the lead has accepted it)

A failed members read says nothing about the destroy.

**When it costs.** The host refuses every command while four children
are left (host.rs:54). In that state every marked name goes to
`stuck` at its first step: 8 names a round, so the list is full after
8 rounds. From then on, each round forgets the 8 oldest with "destroy
failed; destroy by hand", though no destroy was tried on them.

This needs a backlog (a lowered `--keep`, or the give-ups of M1 falling
due together) and a host in that state, so it is low. It could be
avoided by not sticking a name on the host's own refusal (the STUCK
words).

**A test gap.** No test pins that a failed read counts as a failed
destroy. Plant P2, where a failed read lets the name leave as
`retired` with nothing destroyed, survives at d68471f and at e6530f1.
`review179_g_r12_a_members_read_that_fails_keeps_the_name` in
`scratch-retain-tests.rs` kills it.

### (c) Up to 64 retries a round, outside STEPS_MAX: settled by 5caf2ef

At d68471f, a round did up to 64 members reads and 64 or more destroys
besides its 8 steps. That broke the module's own rule that a round
"does not hold one round up" (retain.rs:75-77), and it widened M2.

5caf2ef bounds the retries at 8 a round. With the 8 steps, a round
destroys for at most 16 names. M2 remains: nothing ends those early.

### (d) The changed test is not a weakening

At adc391a it asserted that the name was kept among the published with
`retire=members`. It now asserts that:
- the name has left `published`;
- it is in `stuck` with its members;
- after the refusal clears, `retired` is told, its `@t` are gone, and
  `stuck` is empty.

That is stronger: it adds the retry path. Plants N1 (no retry), N8 (no
stick for a published name), N10 (a retry that succeeds stays stuck)
and P1 (R11 again: a destroy that fails lets the name go) each fail it.

### Record versioning (d68471f)

- **v3 changed in place.** That is fair, since v3 was never released:
  it is new in 39ef4eb, in this same pull request. The golden bytes were
  regenerated, and the only change is the appended entry.
- **Reading v2.** A v2 record still reads, with no names published and
  none stuck (record.rs:427). Plant M6, a v2 record refused, is killed.
- **Records of the first head.** A v3 record written by adc391a would
  not read at the head: postcard is positional, and the new field is
  missing (by reading; not run). Only CI's rigs wrote such records.
- **Downgrade.** There is none: a v2 agent refuses v3, which is the
  versioned-format rule.
- **The record's rules.** `check()` enforces the four new ones (plants
  N15 to N18 are killed).

## The turn (5caf2ef) and its docs (e6530f1)

**What I was asked, and what I found.**

- **Is a name starved across forgotten names, or when a name leaves
  during a turn? No.**
  - The turn is a name compared by value, not a position. The next
    round starts at the first name greater than the last one tried,
    and wraps to the first when none is.
  - So a name that is forgotten, retired, or that was itself the last
    one tried changes nothing for the others.
  - A name that comes into the list behind the turn waits for the wrap,
    at most one cycle: 8 rounds at the cap.
  - A name that comes in ahead of it is retried in the same cycle.
    Such a name can be a newly stuck one, tried a moment ago, so
    "round-robin from the one tried longest ago" holds only for a list
    that does not change.
- **A new process starts at the first name.** A process restarted every
  k rounds therefore retries only the first `8 × k` names by name.
  - Names past those are tried again only once earlier ones leave:
    retired when their destroy succeeds, or forgotten at the cap when
    new names stick.
  - If the first `8 × k` are blocked for good (a person's clones) and
    no new names stick, the later names are never retried while the
    restarts go on.
  - Their snapshots stay, and a name whose clone was removed is not
    retired. Nothing else is affected: stuck names count for no room,
    the record keeps its rules, and the publishes go on.
- **The in-process turn is acceptable.** Restarts more often than
  every 8 rounds mean a crash loop or a flapping volume, where
  retention's space is the least of it.
  - A cheap change would avoid the case without a save: start a new
    process's turn at an offset from the wall clock, for example the
    minute modulo the list's length, instead of at the first name.
  - That is not needed for the merge.
- **`Agent::restart` keeps the turn**, an engine restart within the
  process after an error (engine.rs:752-765). That is consistent with
  "kept in the process". But the docs' "a restart begins it again"
  means a new process, and the word clashes with `Agent::restart`. A
  nit.

**The new docs sentence (e6530f1).** It is right in substance, but not
exact in two places. A nit, not blocking.

1. **The threshold depends on the list.** With n names stuck and a
   restart every k rounds, the first `min(n, 8 × k)` are retried.
   "More often than every `STUCK_MAX / STUCK_STEPS` rounds" is the
   threshold only for a full list. With 20 names stuck, a restart every
   3 rounds still retries them all.
2. **"No other effect" is a little strong.** The later names'
   snapshots stay on the pool, and a name whose blocker was removed is
   not told `retired`. Also, where a give-up stuck late sorts before
   published names stuck earlier, a published name can reach the cap's
   forget, and its "destroy by hand" line, with no retry since it stuck.

   I would write "this has no effect on the room or the publishes".
3. **A style point.** The sentence's line is 100 columns (retain.rs:47),
   where the paragraph wraps at 72. "The first names by name" are the
   oldest, since the names carry their time.

**The test** `the_names_stuck_are_tried_a_few_a_round_in_turn` pins:
- the cap: each round tries exactly 8;
- none twice in a round;
- in the 8 rounds after the first, each name exactly once, which
  includes the wrap: the ninth round goes back to the first names.

The plants at e6530f1 are below (Q1 to Q9, Q2x and Q7x).

## Decisions for the lead

1. **M1 (blocks in my reading).** Accept it, or choose a fix: give-ups
   retired early when room is short, a larger bound, or no begin while
   `stopped`.
2. **M2 (blocks in my reading).** Fix it here, or name it a follow-up.
   The change is small.
3. **L3.** How a person settles a name stopped `other`: docs or a verb.
4. **(b).** Whether the host's own refusal should stick a name.
5. **At `--keep 1000`, PUBLISHED_MAX (1064) leaves no slack.**
   - A backlog of 56 marks with 8 publishes under way gives Full, and
     so do 64 marks with none under way.
   - Marks pile up only when publishes land but retires are not
     answered.
   - Possible fixes: PUBLISHED_MAX = KEEP_MAX + RETIRING_MAX +
     UNDER_WAY_MAX + 1, or a smaller cap on `--keep`.
6. **After an upgrade from v2, `published` is empty.** So names
   published before the upgrade are never retired by the agent. Say so
   in the operator's docs, or accept it.
7. **`left_by` at Intent, with no snapshot seen, gives `Left::Nothing`**
   (engine.rs:2214-2219). A snapshot whose `zfs snapshot` was killed
   at its limit but went through later is then left with no line. This
   is low.

## Test gaps: plants

Each plant is a textual mutant planted alone. Then `cargo test` runs
on the crate it touches (`plant.py`, a 600 s limit), and the file is
restored.

### At adc391a (`plants.json`, `plants.out`): 55 of 75 killed

The baselines passed first:

| crate | passed |
|---|---|
| `skyzfs-agent --lib` | 324 |
| `skyzfs-agent-cli`, test hooks | 7 |
| `skyzfsd --lib`, test hooks | 203 |
| `skyzfs-ublk --lib` | 161 |
| `skyzfs-upload` | 208 |

The plants that survive, and what kills them:

| plant | what it plants | killed by |
|---|---|---|
| R12 | a members read that fails lets the name leave | G-R12 (scratch); it is P2 at the later heads |
| R15 | a look answered `unreadable` is taken as absent | G-R15 (scratch) |
| R16 | a look not answered is taken as absent | G-R16 (scratch) |
| R16b | a look answered otherwise (a refusal) is taken as absent | G-R16b (scratch) |
| R21 | a name given up has `retired` told after it leaves the record | G-R21 (scratch) |
| R22 | `forgotten` is told after the name leaves the record | G-R22 (scratch) |
| E4 | `left_by`: a take leaves `Unknown`, not its members | G-E4 (scratch) |
| E3, E4b | `left_by` at Intent, with and without a snapshot seen | nothing |
| E8 | `begin` checks only the bound on publishes under way, not `room()` | nothing (the first part of T5 would) |
| G1 | the cap counts every `retry`, not only one after a reopen (reading 4) | nothing |
| G3 | only one capped reopen, not one every 10th round (the test runs 20 rounds) | nothing |
| G6 | the quick retries are not counted afresh after another verdict | nothing |
| G12 | a new note does not start the counts again | nothing |
| G9, G10 | retention runs after news on the note, or after an end was asked (program.rs:1669) | nothing |
| G11 | `run` never retains | nothing: no test shows that `run` retires anything |
| G16 | the CLI does not pass `--keep` to the agent | nothing |
| U1, U2 | the uploader counts `gone` as retired, or counts no retire (status v20's counters; skyzfs-upload publish.rs:626, driver.rs:459) | nothing, in `skyzfs-upload`, in `skyzfsd` with its test hooks, or in `skyzfs-agent-cli` (`plants-u.json`, `plants-u.out`) |

- **The scratch tests.** The seven tests in `scratch-retain-tests.rs`
  each fail with their plant and pass on the head
  (`scratch-tests-*.out`). The author may take them as they are.
- **The gaps that matter most.**
  - G11 and G16: no test ties retention to `run`, or `--keep` to the
    agent, and no rig has run retention yet.
  - G9 and G10, whose guard is the one M2 asks to extend.
  - U1 and U2: status v20's new counters are read and laid out under
    test, but how they are counted is not.

### At d68471f (`plants2.json`, `plants2.out`): 27 of 31 killed, 1 that does not compile

- **The author's 18 (N1 to N18).** I made them again in my own text, and
  all 18 are killed, as the description says.
- **Mine on the cap, the forget order, the v2 read and the retry.**
  - Killed:
    - M1 and M2, the cap one high and one low;
    - M3, the retry within the round's steps;
    - M4, only the oldest retried;
    - M6, a v2 record refused;
    - M9, the newest end forgotten.
  - The forget order (the line before the save) is N5, which is killed.
  - M8 (the dump's line) does not compile. M8b is its compiling form,
    run at e6530f1.
- **They survive:**
  - **M5:** a retry that succeeds clears every stuck name. The others
    would then be dropped with no line, their `@t` left. No test has
    two names stuck where one's retry succeeds and the other's fails.
  - **M7:** a stuck name keeps only its first member. A retry then
    destroys that one and tells `retired`, while the other members'
    `@t` stay. The tests' stuck names have one member left by then.
  - **P2** (R12 again): a members read that fails lets the name leave
    as `retired`, with nothing destroyed. So point (b), that a failed
    read counts as a failed destroy, is not pinned.
    `review179_g_r12_a_members_read_that_fails_keeps_the_name` kills it
    (`scratch-tests-d68471f.out`).
- **The other plants of adc391a again.** P1 (R11), P3 (R18) and P4 (C1)
  are killed. R15, R16 and R16b survive at d68471f as at adc391a.

### At e6530f1 (`plants3.json`, `plants3.out`): the turn, and the survivors again

The baseline: `skyzfs-agent --lib`, 330 passed.

**The turn (Q).** `the_names_stuck_are_tried_a_few_a_round_in_turn`
kills each of these:

| plant | what it plants |
|---|---|
| Q1 | the turn starts again at the first name each round |
| Q2 | the turn starts at the last name tried, not after it |
| Q3 | the cap one high (9 a round) |
| Q4 | the cap one low (7 a round) |
| Q5 | no cap |
| Q6 | the turn moves only past a name whose destroy succeeded |
| Q7 | the turn moves by one name a round |

**The exact-once assertion is the one that catches Q2 and Q7.** With
it taken out, Q2x and Q7x survive: those plants try 8 a round, never
the same name twice in a round, and yet skip some names and repeat
others across the cycle. So the assertion carries weight.

**They survive:**
- **Q8:** no sort, so the list's own order (the oldest stuck first).
  In the test the two orders are the same. They differ once a give-up
  is stuck after published names, since its name is older. Unsorted,
  `position(n > last)` can then skip or repeat names. This is a low
  test gap: seed the stuck names out of name order.
- **Q9:** the turn starts again at an engine restart within the
  process. Nothing pins that `Agent::restart` keeps the turn. Either
  way would fit the rulings.
- **M5, M7 and P2** survive as at d68471f. M8b, the dump's line, is
  killed. N1, N10, N11 and N12 are killed again.

**The survivors of adc391a, again at e6530f1.** R21, R22, E3, E4, E4b,
E8 and G11 survive. R15, R16 and R16b survived at d68471f, and the code
they plant has not changed since. G1, G3, G6, G9, G10, G12, G16, U1
and U2 are in code and tests that no later commit touches.

## By commit

- **c7790db, the next look at #175.** Nothing found.
  - #175's L2, L3/D5, L4, L5, G6 to G8, G10 and the nits are addressed.
  - The fake daemon's retire matches skyzfsd: plants F1 and F2 are
    killed.
  - The `ended` advice is `look`: plant X5 is killed.
  - #175's L5 (no lease row) holds: plant U3 is killed.
- **39ef4eb, retention, record v3, status v20, and the sync past
  against `rewound=R`.**
  - Defects: M1, M2, L2, L3, L4 and L5, and L1 until d68471f.
  - Test gaps: plants R12, R15, R16, R16b, R21, R22, E3, E4, E4b, E8,
    G9, G10, G11, G16, U1 and U2 survive.
  - Status v20's layout and its reading are sound: plants S1 to S4 are
    killed. The uploader's count of retires is untested: U1 and U2
    survive.
  - The sync past is sound: plants E6 and E7 are killed.
- **09f7aaf, #173's lows.** No defect found.
  - D1 is fixed: plant G7 is killed.
  - The quick retry and the cap are pinned on their main paths: plants
    G2, G4, G5, G8 and G13 to G15 are killed.
  - Their rules of counting are not: plants G1, G3, G6 and G12 survive.
  - I agree that the author's `held-reads-nothing` is equivalent on the
    fake.
- **c420ad3, tests for the author's surviving plants.** Nothing found.
  My R20 (names given up take no step) is killed by one of them,
  `a_backlog_given_up_is_worked_off_a_few_names_a_round`.
- **adc391a, the `retire` line's commits.** Nothing found.
  - Plants X1 to X4 are killed.
  - The parse of `found=` is sound (see "Earlier findings, checked").
- **d68471f, the stuck names.**
  - It fixes L1 and widened M2.
  - The questions (a) to (d) are answered above.
  - Its own test gaps: M5, M7 and P2 survive.
- **5caf2ef, the turn.** No defect found. On starvation and the
  in-process turn, see above.
  - Its test pins the turn: Q1 to Q7 are killed, and the exact-once
    assertion carries weight.
  - The sort is not pinned (Q8), nor is the turn kept across
    `Agent::restart` (Q9).
- **e6530f1, docs.** Right in substance, but not exact (above).

## Nits

- **retain.rs:157-158** says `run` calls `retain` "after a round that
  touched the pool and was not failed". It also runs after a failed
  begin, `Full` included, and rightly so: otherwise Full would never
  clear. Only the doc is wrong.
- **retain.rs:44** "a restart begins it again": say "a new process",
  since `Agent::restart` keeps the turn.
- **e6530f1's sentence:** the two points above. Its line is also 100
  columns.
- **skyzfs-ublk/src/status.rs:181-182** list the row at offset 1416
  before the row at 1408.
- **The rulings are cited as "?1" to "?4"** (retain.rs:17, 20, 67, 73;
  engine.rs:376, 2213; plan.rs:478, 482; record.rs:217, 388), ids that
  no doc records. Cite where the lead wrote them, or record them in the
  notes.
- **bins/skyzfsd/src/cli.rs:** the rewrapped help has one line of 86
  columns, longer than its neighbours.

## Earlier findings, checked

- **The next look at #175** (`review/m2.7b1/next_look/`): its L2,
  L3/D5, L4, L5, G6, G7, G8, G10 and the nits are addressed in
  c7790db, as the description says. My review's scratch tests went in
  as they were, renamed.
- **#173's D1**, an early round that uses up its tick, is fixed in
  09f7aaf. Plant G7 brings it back, and three tests kill it.
- **#171's O1**, the sync past a rewind against `rewound=R`, is done in
  39ef4eb. Plants E6 (clear at the mark) and E7 (the mark not read) are
  killed.
- **`found=`:**
  - One prefix, `control::other_commit` beside `retired_reply`, both
    writes and reads it, so the two cannot drift apart.
  - The test builds its text with `retired_reply`, so changing the
    words fails it: plants X1 to X4 are killed.
  - A refusal with no commit, or a short one, gives no `found=`.
- **Status v20.** No reader refuses it, since every reader compares
  with `>=`. v19 still reads (plant S3 is killed).
- **`unwrap()` and `expect()` outside tests.** The diff
  `7cf6da6..e6530f1` adds none.
- **The description.** It no longer calls a stuck name's retry "a
  single `zfs` call", which it was not: it is a members read and a
  destroy for each member.

## Gates and CI

Run with rustc 1.100.0, the current stable, after `rustup update
stable`.

**adc391a, the whole workspace (`gates.out`).**
- `cargo fmt --check` is clean.
- With no features:
  - clippy (`-D warnings`): clean;
  - tests: 2113 passed, 0 failed, 16 ignored;
  - doc (`-D warnings`): clean.
- With `skyzfsd/test-hooks,skyzfs-local/test-hooks`:
  - clippy: clean;
  - tests: 2172 passed, 0 failed, 16 ignored;
  - doc: clean.
- With `skyzfs-ublk-sys/control-test-program`:
  - clippy: clean;
  - tests: 2137 passed, 0 failed, 18 ignored;
  - doc: clean.
- With `skyzfs-agent-cli/test-hooks`: clippy is clean. I stopped the
  tests and the repeats once the head had moved on.

These counts match the description at adc391a.

**d68471f and e6530f1, the agent's crates (`gates2.out`, `gates3.out`).**
Every commit after adc391a touches `crates/skyzfs-agent` alone, so I
ran these gates on `-p skyzfs-agent -p skyzfs-agent-cli`, with no
features and with `skyzfs-agent-cli/test-hooks`. fmt ran on the
workspace.
- clippy and doc are clean at both heads.
- Tests: 336 passed at d68471f and 337 at e6530f1, 0 failed.
- The tests were repeated 5 times at d68471f and 3 times at e6530f1,
  and all passed each time.
- The counts follow the description's: +5 tests at d68471f (2118) and
  +1 at 5caf2ef (2119).

Read with `gh api repos/skyzfs/skyzfs/commits/<sha>/check-runs`.

- **e6530f1, the head.** At the time of writing, 25 checks succeeded,
  3 were skipped, and 22 were still running or queued. None had failed.
  - Done: the unit and log jobs, `ubuntu-24.04`, `ubuntu-26.04` and
    `ubuntu-26.04-arm`, the record and write-into-a-hole jobs through
    skyzfsd, the agent rig's course a1, and the k3s UDS smoke.
  - Still running: the daemon rig (daemon and stand-in), the replay
    rig, the publish rig, the agent rig's courses k, a2, r and d,
    `test`, `unit-arm64`, `lint` and coverage.
  - The lead should read the rigs' tables once they finish.
- **d68471f.** 40 succeeded and 5 were skipped. The daemon rig's three
  jobs were cancelled by the newer push.
- **adc391a.** 42 succeeded and 5 were skipped. "ubuntu-26.04, daemon"
  was cancelled by the newer push.
- **What reaches the head.** No commit after adc391a touches the daemon
  or the uploader, so their rigs' runs at adc391a also speak for the
  head. The agent's code changed after adc391a, and its rig's courses
  at e6530f1 are the ones to read.

## Files

In `review/m2.7b2/`:

- `review.md`: this note.
- **Gates:**
  - `gates.sh`, `gates.out`: adc391a, every feature set;
  - `gates2.sh`, `gates2.out`: d68471f, the agent's crates;
  - `gates3.sh`, `gates3.out`: e6530f1, the agent's crates.
- **Plants:**
  - `plant.py`, the planting script;
  - `mk.py`, `plants.json`, `runplants.sh`, `plants.out`: adc391a;
  - `plants-u.json`, `plants-u.out`: U1 and U2, against `skyzfsd` and
    the CLI;
  - `mk2.py`, `plants2.json`, `runplants2.sh`, `plants2.out`: d68471f;
  - `mk3.py`, `plants3.json`, `runplants3.sh`, `plants3.out`: e6530f1.
- **Scratch tests:**
  - `scratch-program-tests.rs`, `scratch-retain-tests.rs`, `apply.py`:
    T1 to T5, the gap tests, and the script that appends them to a
    tree;
  - `scratch-tests-adc391a.out`, `scratch-tests-d68471f.out`: their
    runs.
