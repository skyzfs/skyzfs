## Review of #127 at 1595d6f (M2.5-2)

The daemon's control channel: a Unix socket `control` in the volume's run
directory, answered on a thread of its own (`bins/skyzfsd/src/control.rs`,
`control/server.rs`, `control/handler.rs`); the commands `cuts`,
`publish`, `publish-progress` and `withdraw`; the uploader answering its
inbox between two turns (`Uploader::run_with`, `control()`); status v18;
the request log's `publish` lines; the kill points `manifest:N` and
`publish:N`. The branch is stacked on #125 (M2.4e) and #126 (M2.5-1b); I
reviewed M2.5-2's own seven commits (8a7efae, 460e67c, 898a3b5, fdd72ac,
58b2e39, b5ad5a0, c5785bc) and not the stack. #126's F1 is not counted
here, as asked.

**Verdict: one finding blocks the merge in my judgement (F1). A channel
request makes a store call that holds back the lease's renewal, so the
lease row can lapse.** This undoes #117's F4 ("a slow turn of the queue
cannot let the row lapse") for any volume someone asks `cuts` of, and
460e67c adds the same gap to every `publish`. The fence keeps it from
being a safety fault, so it costs liveness: a contender can take the row
in the gap. The fix is small. Codex's one finding is real and is my F2
(low, does not block). F3 to F5 are suggestions or documentation, and
there are two test gaps (T1, T2). Apart from these I could not break the
protocol, the socket guard, status v18, the request log's lines or the
kill points.

Decisions 1 to 6: 1, 2, 3, 5 and 6 match the code and I find none of them
wrong. Decision 4's aim is right, but its code is not: it is half of F1,
and in the daemon the state it reads is already the one the uploader was
given at the start (F5). "Serving comes before publishing" holds in the
code. A socket that cannot be made logs a warning, posts `control
failed`, and the serve goes on (`serve.rs:889-937`, and plant A-C3 below
is caught).

CI on the head: everything is green except `skyzfs-cut-daemon` on
ubuntu-26.04 and ubuntu-26.04-arm, which fail at `bound: cuts_abandoned`.
The same line fails on #125's own run, and #126's run, which does not
contain #125, is green. So the failure comes with the stack and not with
these commits (section CI). The daemon rig's state is in the same section.

### What I did

- Read CLAUDE.md, design §5.2 and §9, and the M2.5 rows of the
  implementation notes. Then I read the seven commits' diff, and the
  description only after that, against the code.
- Gates at the head with rustc 1.100.0 (`rustup update stable`), on the
  three crates the commits touch (`skyzfsd`, `skyzfs-upload`,
  `skyzfs-ublk`) and `skyzfs-local`, which they call:
  - `cargo fmt --all --check`: clean.
  - `cargo clippy --all-targets -- -D warnings`: clean, plain and with
    `skyzfsd/test-hooks,skyzfs-local/test-hooks`.
  - Tests, 0 failed. With test-hooks: `skyzfsd` 169 in the library and
    3 in `closed_output`; `skyzfs-upload` 142; `skyzfs-ublk` 160;
    `skyzfs-local` 320. Plain: `skyzfsd` 130 and 3.
  - Not run here: `skyzfs-ublk-sys/control-test-program`, because the
    commits do not touch that crate, and the whole workspace, for lack
    of disk. CI covers both (`ci`'s `lint`, `test` and `unit-arm64`,
    green).
- Scenarios of my own, appended to the test files for a run and then
  removed: six on the socket (`scenarios_control.rs`) and three on the
  uploader (`scenarios_up.rs`). They cover concurrent, silent,
  trickling, oversized and over-long clients, a close with a full
  backlog, a stalled PUT, the state before the first cut, and a slow
  store answer beside the lease.
- 19 textual plants, one at a time, each restored after its run
  (`plants.py`, `plants.out`). Five of them are the author's (C3, K2,
  U2, S2, R2); all five are caught as the description says. Of my 14,
  12 are caught and 2 survive (T1, T2).
- There is no kernel here, so nothing of mine drove the daemon through
  a device, and no rig drives the channel yet.

The worktree of the head was clean after every run.

### The four questions

**(1) The protocol. Sound.**
- The greeting, one request per connection, and the answers are as the
  module docs say.
- `Code::of` maps each of the uploader's refusals to its own code.
- `manifest` above `MANIFEST_MAX` and a bad `expiry-s` are refused
  before any manifest byte is read (plant M14 is caught).
- A greeting of more than `LINE_MAX` bytes is refused `version`, and an
  over-long request line `malformed`. The client gets the refusal even
  when the daemon closed with bytes unread: the client reads past
  `ECONNRESET`.
- A line of exactly 4096 bytes is taken (M3 is caught). Bytes after the
  request are ignored.
- The text of `no-answer` says the right thing: a publish may have
  been taken.

**(2) The socket guard. Sound, but for F2 and F3.**
- The mode is 0600 (M2 is caught).
- The peer check is `uid == 0 || uid == geteuid()`
  (`server.rs:64-66`). It is correct by reading, but no test shows it
  refusing a user (T1).
- One request per connection, one connection at a time: 20 concurrent
  clients were answered one after another in 1.0 s.
- A silent client that sent half a request held the next one for 9.98 s,
  as designed.
- The limit is per read and per write, not per connection, so a
  trickling client holds the channel indefinitely (F3).
- A stuck client never blocks the volume's serving. The channel's thread
  is its own and reaches the uploader only by `try_send`, and
  `Channel::close` is bounded by `CLOSE_LIMIT`, except when the backlog
  is full (F2).

**(3) The uploader's inbox. Between turns as described, but outside the
lease (F1).**
- `run_with` (`uploader.rs:615-655`) answers the inbox only between two
  turns. A publish that is accepted starts the next turn at once.
- Requests whose reply is closed are skipped, and a stop during a
  request ends the run with that request unanswered (the handler says
  `ended`).
- Judging a publish against the store's state when it is asked (460e67c)
  works (M13 is caught). But it is one of the two store calls in F1, and
  in the daemon it reads what the uploader already has (F5).

**(4) Status v18, the request log, the kill points. Sound.**
- v18 is 1408 bytes, with the channel at 1376..1408, and a v17 file is
  still read without the publishes (A-S2 is caught).
- The request log's `asked`, `withdraw` and `ended` lines carry G and C
  in their columns (M12 and A-R2 are caught). A tab in a name is a minor
  point below.
- `manifest:N` counts the PUT of the framed manifest under `snap/`.
  `publish:N` counts only a publish whose transaction landed (A-K2 is
  caught).

### Defects

**F1. A channel request's store call holds back the lease's renewal, so
the lease row can lapse. Blocks the merge. Certainty: high (run).**

`crates/skyzfs-upload/src/uploader.rs:297-299` (`cuts`) and `:312-318`
(`publish`, added by 460e67c) await `local.cuts()`. In the daemon that is
a call to the desk, which the queue's thread answers between its
requests. `run_with` awaits `control()` at `:642-645`, outside
`step_keeping` (`:513`). While the store's answer is awaited, nothing
renews the lease: the renewal comes only at the next turn. #117's F4
fixed this gap for a step, and its fix was a condition of that merge.
A channel request opens the same gap again, and `cuts` is the first
thing a publish asks.

Scenario: `review_a_slow_store_answer_to_a_channel_request_and_the_lease`
in `scenarios_up.rs`. It is #117's scenario C2 with a channel request in
place of the step.
- The lease's time is 30 s, renewed every 10 s.
- A request arrives 0.25 s before a renewal is due, and the store takes
  21 s to answer `cuts`.
- Result, for both `cuts` and `publish`: the lease is taken at 0 s,
  30.75 s, 40.75 s and so on. The row expired at 30 s and was renewed
  0.75 s later.
- With 460e67c's store call removed, `publish` renews at 0, 10 and 20 s,
  on time, and `cuts` still lapses.
- In general the row lapses when the store's answer takes more than
  two thirds of the lease's time. At the shortest lease the daemon
  takes (3 s), that is a little over 2 s.

What it costs: when nobody contends, a late renewal by the same holder
keeps the epoch, and nothing is lost. When another holder contends, it
takes the row in the gap, the epoch moves, and this daemon's uploads are
fenced and stop. The fence keeps it from being a safety fault.

A fix, either of these:
- (a) Await `control()` beside the lease, as `step_keeping` awaits a
  step. For example, generalise `step_keeping` over the future it
  polls.
- (b) Make no store call in `control()`. Answer `cuts` from the
  channel's thread through the desk with its own limit (this also
  answers F4), and drop the store call from `Ask` (F5 explains why the
  daemon loses nothing).

A test like the scenario above, asserting that no two of the lease's
takes are more than the lease's time apart, would hold either fix.

**F2. `Channel::close` and `Drop` can block forever when the socket's
backlog is full. Does not block the merge. Certainty: high (run). This
is Codex's P2.**

`bins/skyzfsd/src/control/server.rs:149` and `:167` wake the thread with
a blocking `UnixStream::connect`. AF_UNIX `connect` waits while the
listener's queue is full, and the queue holds up to `somaxconn` (4096
here).

Scenario: `review_close_with_a_full_backlog` in `scenarios_control.rs`.
It is ignored by default, since it hangs.
- One client trickles a byte every few seconds (F3).
- 4097 more connections queue behind it.
- `close()` had not returned after 20 s, and it returns only once the
  trickler stops.

`serve` calls `close()` (`serve.rs:1690`) before the uploader's thread
ends and the lease is released. So a stop of the daemon hangs with it.

It needs a peer of the daemon's own user or root that misbehaves. A fix:
wake with a non-blocking connect (`EAGAIN` is fine, since a full queue
already holds connections that will wake `accept`). Then `CLOSE_LIMIT`
bounds the close as its documentation says.

### Suggestions

**F3. No deadline per connection. Certainty: high (run).**

`server.rs:192-193` sets `IO_LIMIT` on each read and write.
`control.rs:20` says "a connection that sends nothing for `IO_LIMIT` is
closed", which is true as written. But a client that sends one byte
every 9 s holds the only thread indefinitely: a request line can take
4096 × 9 s, and a manifest of up to 1 MiB longer still.

Scenario: `review_a_trickling_client_holds_the_channel_past_io_limit`.
A client behind the trickler, with a limit of 12 s, got no answer
(`WouldBlock`). With the backlog not full, `close()` returned `false`
after 5.0 s, as documented.

Suggestion: one deadline for reading the whole request (for example
`ASK_LIMIT`), with the read timeout set to the time left before each
read.

**F4. `cuts` waits for the step under way, and a stalled PUT has no time
limit. Certainty: high (run).**

The uploader answers between turns, and the S3 client sets no operation
timeout. So while object storage stalls, `cuts`, a read of the store's
own state, is answered `no-answer`.

Scenario: `review_the_count_waits_for_the_step_under_way`. Each PUT of
the step is stalled 60 s. The count was not answered within 30 s, and an
unbounded wait was answered at 118 s.

Suggestion: answer `cuts` without the uploader (F1's fix (b)). Also,
time limits on object storage operations would help more than the
channel.

**F5. "None before its first cut" is wrong. Certainty: high (run).**

The CLI's help (`bins/skyzfsd/src/cli.rs:347-348`) and the doc of
`Publishes::state` (`bins/skyzfsd/src/upload.rs:805`) say the store's cut
state is none before its first cut. In the daemon it is named from the
start. `InPlace` begins or takes up a cut state at its start whenever it
holds cuts, and an uploading daemon always does. Scenario:
`review_the_cut_state_is_named_before_the_first_cut`.

Two consequences:
- `state=none count=0 group=none` is answered only from a closed desk
  (`crates/skyzfs-upload/src/queued.rs:320-322`), that is, once the
  store has ended. That answer reads like a fresh volume, where
  `refused ended` would say what has happened.
- 460e67c's premise does not arise in the daemon: `Publishes.state` is
  the store's state from the start (`serve.rs:1197`), and it changes
  only at a start. Its store call can go (F1) without changing what the
  daemon answers. The test that 460e67c added would then give the state
  at the start, as the daemon does.

### Minor

- The socket is made after the `ready` line: `serve.rs:1491` prints the
  line and `:1574` opens the channel. A script that waits for `ready`
  and then runs `skyzfsd cuts` can find no socket. The help says "made
  once the device serves", which a reader takes to mean `ready`. Either
  make the socket first, or say to wait for `control` in the status.
  (By reading; not run.)
- A tab in a name breaks the request log's columns. `withdraw name=a<TAB>b`
  parses (`control.rs:416-420` accepts any word), and `record.rs:293-295`
  writes the name raw into a tab-separated line. This affects the
  test-hooks build only. Checking the name with
  `skyzfs_upload::publish::valid_name` at parse, and refusing `name`,
  would close it. (By reading.)
- An `accept` error is retried at once (`server.rs:187-191`). One that
  persists, such as `EMFILE`, spins the thread at full CPU. A short pause
  would avoid that. (By reading.)

### Test gaps

- **T1 (M1 survives).** Nothing shows the real peer check refusing a
  user: the tests pass their own `allowed`. A unit test of
  `same_user_or_root(geteuid() + 1) == false` would hold it.
- **T2 (M5 survives).** No test checks the `no-upload` reason when
  another node holds the lease (`serve.rs`, `no_upload_why`,
  `AtStart::Another`).

### Plants

| Plant | Result |
|---|---|
| M1 the peer check lets any user in | survives (T1) |
| M2 the socket keeps the umask's mode | caught |
| M3 a line of exactly `LINE_MAX` refused | caught |
| M4 a full inbox answered `ended` | caught |
| M5 the reason for another holder dropped | survives (T2) |
| M6 `version` refusals not counted as malformed | caught |
| M7 a refused publish exits 0 | caught |
| M8 the newest cut always opens as itself | caught |
| M9 `take_ended` does not drain | caught |
| M10 the attachment not told to the uploader | caught |
| M11 a peer refusal not written | caught |
| M12 an `asked` line with G and C swapped | caught |
| M13 the store's state not read at an ask (460e67c undone) | caught |
| M14 the early expiry refusal dropped | caught |
| A-C3 a failed channel not posted `failed` | caught |
| A-K2 `publish:N` counts a refused publish | caught |
| A-U2 the status leaves the publishes out | caught |
| A-S2 a v17 file read with a channel | caught |
| A-R2 a withdrawal has no line | caught |

The tests that catch each plant are in `plants.out`.

### CI and Codex

Runs on 1595d6f:
- Green: `ci`, `e2e`, `k3s-smoke`, `coverage`, `skyzfs-ci` (push),
  `skyzfs-replay-rig`, `skyzfs-control-rig`.
- `skyzfs-daemon-rig`: DAEMON_RIG_STATE
- `skyzfs-cut-daemon` (run 44401327161): fails on ubuntu-26.04 and
  ubuntu-26.04-arm, and passes on ubuntu-26.04-upload. The only FAIL is
  `bound: cuts_abandoned: FAIL: '0' abandoned at the bound, with the
  copies '0' bytes of '268435456'`.

The same line fails on #125's own run at da2b697 (run 44382978577, the
same two jobs). #126's run at ec02de6 (run 44394476624), which does not
contain #125, is green on all three images. So the failure comes with
#125 and not with these commits. The mechanism is my guess and not
checked: if writes into hole chunks during the first cut make no
copies (M2.4e's hole set), the copies never reach the bound. It is
#125's and testing's to settle. Until it is settled, this head is not
green.

Codex reviewed 1595d6f and left one finding, P2 on `server.rs:150`,
"Wake the control thread without a blocking connect". It is real, and it
is F2.
