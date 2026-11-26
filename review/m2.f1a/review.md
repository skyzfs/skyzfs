## Review of #166 at 7d98b26 (F1(a): skyzfsd drops a control request whose client hung up)

**Verdict at 7d98b26: not yet. One finding blocks: T1, a flaky test.** Its fix is a few lines in the test; the product code is right.

- **The check is the right one** (1). `POLLHUP` or `POLLERR`, with `POLLRDHUP` not asked for, is what "the client has gone" means on a Linux Unix stream socket.
  - A half-close is answered. A client that closed, before or after its connection was accepted, is dropped.
  - I watched every case on a real kernel (6.18), with a probe of `poll(2)` and with tests of `hung_up` itself.
- **Refusals unchecked is right, and consistent** (2). A refusal does nothing. A refusal to a gone client fails to write and goes uncounted, as a dropped request does.
- **The new section of `control.rs` claims nothing the code does not do** (3). Its last paragraph is the conservative reading, and it is too narrow in two places (D2).
  - "Only a deadline carried in the request closes that" is too strong. Watching the socket while the handler waits would close most of that window, with no change to the protocol.
  - A client that stopped waiting but did not close, such as an agent that was stopped, is caught by neither sentence.
- **The tests catch the plants they name** (4), and the author's five are killed as the description says.
  - One test is flaky: T1. Its count of answers races the server's count, and it failed 9 times in 401 runs here, one of them a plain gate run.
  - The half-close test catches its mutant only as long as the client's `shutdown` beats the server's `poll`: T2, a nit.
- **For the lead, as context.** With this change, a publish the agent sent is accepted at most 80 s after its stamp. The agent's hold runs to 90 s plus the expiry. So the chain of #149's F1 is closed for the agent as built, except where the agent stops without closing its connection. That case is what F1(b) is still for (under D2).
- **Gates at 7d98b26** (rustc 1.100.0, the current stable):
  - fmt, clippy (skyzfsd plain, skyzfsd with `test-hooks`, the workspace) and `cargo doc -D warnings` (the same three) are clean.
  - Tests: skyzfsd plain 147 passed and **1 failed (T1)**. skyzfsd with `test-hooks`: 203 passed. The workspace: 1967 passed, 0 failed, 16 ignored, the description's count.
- **CI on 7d98b26 is all green**, and the daemon rig's tables agree with the colour:
  - 87 rows on each of ubuntu-24.04, 26.04 and 26.04-arm, every one as the table `EXPECTED` has it. The pin of a held request is WAITING, as expected.
  - The publish rig drove a half-closed raw `cuts` through skyzfsd and the kernel, and it was answered (D1).
- **Codex** completed its code review and its security review of 7d98b26 with no findings.

**Findings, most severe first.**

| Id | Severity | Where | What | Certainty |
|---|---|---|---|---|
| T1 | **blocks**: a flaky test | `bins/skyzfsd/src/control/tests.rs:504` | `assert_eq!(channel.figures().answered, 2)` reads the count right after the last client's `ask` returns. But the server counts after it has written the answer (`control.rs:163-164`, then `server.rs:328`), and `ask` returns once it has read the answer line. It failed 9 of 401 times here. | high |
| D2 | low: the boundary as written | `bins/skyzfsd/src/control.rs:44-51` | "Only a deadline carried in the request closes that" is too strong. A client that has stopped waiting but has not closed is in neither sentence. | high (the mechanism); the alternative is not built |
| G1 | low: a gap | `server.rs:327`, `control.rs:154-159` | A dropped request leaves no trace at all: no count, no line on standard error, no line in the request log. The description proposes a counter for later, which needs a new status layout. A line on standard error needs none. | high |
| G2 | low: documentation | `crates/skyzfs-control/src/lib.rs:9-12`, `:31-35` | The protocol's own documentation, which an author of a client reads, does not say that a request whose client closes before it is handled is dropped. Only `skyzfsd`'s module says so. | high |
| T2 | nit: a test's reach | `bins/skyzfsd/src/control/tests.rs:515-533` | The half-close test shuts the client's writing down after the server has the connection. So a mutant that takes the half-close for a hang-up is caught only when the `shutdown` lands before the server's `poll`. P2 was caught 100 of 100 times, but Q4 (`POLLIN` taken as gone, which has the same race) only 99 of 100. | high |
| D1 | the description | "CI" and "Not done" | The CI line is a placeholder ("will link the runs"), and every run on 7d98b26 has ended green. "No rig here shows the drop through the kernel" is right. But the publish rig does show the other half through the kernel: a half-closed raw `cuts` answered (`pin_answer_cuts`, `control_answered 4`). | high |

### T1 (blocks): the count of answers is read before the server makes it

**The race:**
- `answer` writes the answer and flushes it (`control.rs:163-164`). Only then does `serve` add to `answered` (`server.rs:328`).
- The client, `skyzfs_control::ask`, returns once it has read the greeting and the answer line. It does not wait for the server to close (`crates/skyzfs-control/src/client.rs:40-69`).
- The test joins the waiting client, then reads `channel.figures().answered` at once (`tests.rs:499-504`). If the server's thread has not reached `fetch_add` yet, the count is 1.
- The older tests do not have this race:
  - `the_channel_answers_through_its_socket` reads the figures after `read_to_string`, which ends only when the server drops the stream, after the count.
  - `the_channel_posts_its_figures_after_each_connection` waits for the posts.

**Evidence** (`flaky_head.out`, `gate_counts.txt`):
- The plain gate run (`cargo test -p skyzfsd`) failed with `left: 1, right: 2` at `:504`.
- Alone, with the head's own test binary:
  - 3 of 100 runs failed at the ambient load (other reviews' builds, load about 7 on 4 cores);
  - 1 of 100 and then 4 of 200 failed beside 4 busy loops.
- The timing probe T1 (`plants166.py`) changes no behaviour: it makes the count 300 ms after the answer. With it, this test fails, and every other test passes.

**Why it blocks.** It is a red CI waiting to happen on any later pull request that touches skyzfsd, on a loaded runner. #150's B1, a flaky test, was ruled blocking. The test passed on this head's CI runs, and that is luck at about 2%.

**The fix** (`r5_the_queued_publish_test_with_the_posts_awaited` in `review_tests.rs`):
- Open the channel with `Channel::open_posting` and a channel for the posts.
- After the joins, take two posts, with `answered` 1 and then 2, and assert that no third comes within 300 ms.

I ran this form:
- 0 of 200 runs failed beside 4 busy loops.
- It passes under the timing probe.
- It is killed by P1, P4, Q1 and Q7, the plants the original catches through this assertion (`plants_r5.out`).

Reading the figures in a loop until they reach 2 would also stop the flakiness. But that loop cannot see Q7, a dropped request counted: the count then passes 2 on its way to 3.

### (1) Is `POLLHUP | POLLERR`, without `POLLRDHUP`, the right test?

**Yes.** What Linux reports on the accepting end of an `AF_UNIX` `SOCK_STREAM` connection, polled after the request was read whole as skyzfsd does (`poll_probe.py`, `poll_probe.out`, kernel 6.18):

| The client | `events=0` (what `hung_up` asks) | `events=POLLRDHUP` | `hung_up` | A write of the answer |
|---|---|---|---|---|
| sent, still connected, waits | 0 | 0 | present | ok |
| sent, then `shutdown(SHUT_WR)` (half-close) | 0 | RDHUP | present | ok |
| sent, then `close()` | HUP | HUP, RDHUP | gone | EPIPE |
| sent, then `close()`; polled before the server reads | HUP | HUP, RDHUP | gone | EPIPE |
| sent more than the request, then `close()` (bytes left unread) | HUP | HUP, RDHUP | gone | EPIPE |
| sent, then `shutdown(SHUT_RD)` only | 0 | 0 | present | EPIPE |
| sent, then `shutdown(SHUT_RDWR)`, not closed | HUP | HUP, RDHUP | gone | EPIPE |
| queued: sent and `close()` before the accept | HUP | HUP, RDHUP | gone | EPIPE |
| queued: sent and `SHUT_WR` before the accept | 0 | RDHUP | present | ok |
| the server wrote first, and the client closed with that unread | ERR, HUP (`SO_ERROR` ECONNRESET) | ERR, HUP, RDHUP | gone | EPIPE |
| a forked child still holds the client's descriptor, and the parent closed | 0 | 0 | present | ok |
| the client process killed (SIGKILL) after sending | HUP | HUP, RDHUP | gone | EPIPE |

What this shows:
- **`POLLRDHUP` is reported only when asked for.** The kernel masks the result with the events asked plus `POLLERR | POLLHUP`. The code asks for nothing, so `RDHUP` can never be seen, and plant Q2 (`RDHUP` taken but not asked for) survives as an equivalent mutant.
  - Taking `RDHUP` would be wrong. The rig's own raw client, `tests/kernel/publish_agent.py:246-247`, sends and then shuts its writing down, and so does any client that marks the end of its request that way.
  - The publish rig drove exactly that through skyzfsd on this head. Its `pin_answer_cuts` came back `cuts attachment=...`.
- **Unread data in the server's buffer does not hide the hang-up.** `HUP` is set, and `IN` beside it.
- **`POLLERR` never comes alone on this socket.** `sk_err` is set only when the peer is released with bytes unread on its own side, and the same release sets `SHUTDOWN_MASK`. skyzfsd writes nothing before the poll, so in practice `ERR` comes only with `HUP`. Plant Q3 (`HUP` alone taken) survives for that reason, and keeping `ERR` costs nothing.
- **A client that sent and then closed, meaning the request to run, is now skipped.**
  - Every client in the tree waits for its answer: `skyzfs_control::ask`, used by the agent and by `skyzfsd ask`, and the rig's raw client, which half-closes.
  - The agent's `ask` closes only at its deadline, by returning, and that is exactly the give-up F1 is about.
  - For the two requests with an effect, the drop is safe from the agent's side. A publish it gave up on and a withdraw it gave up on are both treated as not known to have happened: the record stays `asked` (`crates/skyzfs-agent/src/engine.rs:1472-1473`), and a withdraw whose ask failed is `Moved::No` and is asked again (`engine.rs:980-994`). A drop is the same to the agent as a request that never arrived.
  - The skip is silent, though (G1), and the protocol's documentation does not state it (G2).
- **Where the poll is.** It comes after the last byte of the request (the manifest included) and right before the handler (`control.rs:152-160`). That is the latest point before any effect.
  - The gap between the poll and the handler's start is microseconds.
  - The window that matters is the handler's own: up to `ANSWER_LIMIT` (30 s) waiting for the desk and the uploader. The docs leave it to F1(b); see D2.
  - Placing the poll before the read (Q5) is caught, though only incidentally: by the refusal case of `a_request_whose_client_hung_up_is_not_handled`.
- **No busy loop, and no EINTR problem.** The poll has a zero timeout and runs once per request. With a zero timeout, `do_poll` returns `EINTR` only when no event is ready, so the `Err(_) => false` that answers it is the right verdict. Q9, a 50 ms wait, survives: it would only add latency.
- **A descriptor shared with another process keeps the connection open** (the forked-child row). That is not the agent's case: Rust's sockets are close-on-exec, and the agent does not fork without exec.

### (2) Refusals are not checked

**Right, and consistent.** A refusal changes nothing, so a gone client loses nothing by it.
- For a gone client the refusal's write fails with EPIPE. `answer` returns the error, and `serve` counts nothing, neither `answered` nor `malformed`. A dropped request is counted the same way.
- My test `r3_a_withdraw_and_a_refusal_whose_clients_gave_up` shows both, through the posts: a malformed request from a client that closed is neither answered nor counted.
- The peer check (`server.rs:311-320`) also refuses without looking at the client. P5, refusals looking at the client, is killed by the panicking check in `a_request_whose_client_hung_up_is_not_handled`.

### (3) The new section of control.rs, and D2

Each claim against the code:

| Claim (`control.rs`) | Holds? |
|---|---|
| Linux keeps a queued connection, with its request, after the client closes (`:30-32`) | yes: probe rows "queued", and test `r1b` |
| A queued publish reached the handler 1.47 s after its client gave up, on the real channel (`:34-36`) | yes, as #149's review measured it |
| Dropped "by the time the request is read whole", before the handler (`:36-38`) | yes (`:152-160`) |
| Not answered, not counted in `control_answered`, no line in the request log (`:38-39`) | yes: nothing is written; `serve` counts only `Ok`; the request log's lines come from the handler's wrapper (`record::channel`, test builds), which is not asked |
| `POLLHUP` once the peer has closed (both ways); a client that only shuts its writing down is answered (`:39-42`) | yes: probe, `r1`, `r2` and the rig's raw `cuts` |
| Not a client that gives up while its request is handled; the handler's limit runs from when it is asked (`:44-49`) | yes: `r4_a_client_that_gives_up_while_handled_is_still_handled` pins it |
| "Only a deadline carried in the request closes that" (`:49-50`) | **too strong** (D2a) |
| A `poll` that fails is taken as a client still there (`:50-51`) | yes (`server.rs:162`). Q6, a failing poll taken as gone, survives: no test can make `poll` fail |

**D2a. A deadline is not the only way.**
- The handler waits on a `oneshot` for the uploader's answer (`bins/skyzfsd/src/control/handler.rs:223`, `wait`).
- The uploader skips a request whose answer channel is closed (`crates/skyzfs-upload/src/uploader.rs:411-413`), and it takes the request synchronously (`control` waits on nothing).
- So a handler that watched the client's socket while it waited, and dropped the receiver at a hang-up, would close most of the window. That needs no change to the protocol, only an internal one to the handler's signature.
- What would be left: a hang-up between the uploader's `is_closed` and its `ask`; a stopped client (D2b); and the desk's wait before the send, unless it is watched too.
- A deadline in the request closes all of it. So "only a deadline closes it fully" is accurate, and "only a deadline closes that" is not.
- I did not build this; it is an option for the lead's scoping of F1(b).

**D2b. A client that stopped waiting but did not close.**
- The check sees a close, not a deadline. An agent stopped (SIGSTOP, a frozen cgroup) after it sent and before its deadline keeps its socket open. Its request is handled whenever its turn comes, however late.
- When it resumes past its deadline, its `ask` returns `TimedOut` without reading the answer: `Deadline::read` checks the time first (`client.rs:168-174`, with `left` at `:80-86`). So the agent can be told nothing of an accepted publish.
- Neither sentence of the section covers this. One clause would: "a client past its own budget that has not closed, such as a stopped process, is still served; only a deadline in the request catches it".

**For the lead: what F1(a) bounds.** This is from the code; I did not run it.
- The agent stamps `sent`, and sends within `ASKING_SLACK` (10 s; `engine.rs:1435`).
- Its `ask` closes the connection at its deadline, `ASK_LIMIT` (40 s) after it begins. So skyzfsd reads and handles a request only while the client is connected: at most `sent + 50 s`.
- The handler waits at most `ANSWER_LIMIT` (30 s), and the uploader skips a request whose handler gave up (`uploader.rs:411`). So a publish is accepted by `sent + 80 s`.
- Its expiry runs from the acceptance (`crates/skyzfs-upload/src/publish.rs:700-702`), so it ends by `sent + 80 s + E`.
- The record's `held_until` is `sent + 10 + 40 s + E + HOLD_MARGIN (40 s)`, that is `sent + 90 s + E` (`engine.rs:1406-1409`, `plan.rs:477-482`).
- So the hold outlasts the request by at least 10 s. #149's F1 chain is closed for the agent as built, for a client that closes at its deadline.
- What is left for F1(b): the stopped client (D2b), and a margin of 10 s.
- The doc of `ASKING_SLACK` (`crates/skyzfs-agent/src/plan.rs:284-288`, "accepted, if at all, by `ASK_LIMIT` after") is still short by `ANSWER_LIMIT`: #149's nit 4. The 40 s margin absorbs it. That is the agent's text, not this pull request's.

### (4) The tests, and the plants

Each plant was put in alone, as text, in a clean worktree of the head that also held my scratch tests. Each was run against `cargo test -p skyzfsd --lib control::` with a 600 s limit and restored in a `finally` (`plants166.py`, `plants.out`). The worktree was clean afterwards.

| Plant | What | Verdict | Caught by the PR's tests | By mine |
|---|---|---|---|---|
| P1 | the client not looked at | KILLED | `a_request_whose_client_hung_up_is_not_handled`, `a_publish_whose_client_gave_up_in_the_queue_is_dropped` | r3, r5 |
| P2 | `POLLRDHUP` asked and taken | KILLED | `a_client_that_only_stops_writing_is_answered` (100 of 100 in repeats) | r1, r2 (100 of 100) |
| P3 | only `POLLERR` taken | KILLED | the queued-publish test | r1, r1b, r3 |
| P4 | looked at after the handler | KILLED | both | r3, r5 |
| P5 | refusals look at the client | KILLED | `a_request_whose_client_hung_up_is_not_handled` | - |
| Q1 | the server's closure always says present | KILLED | the queued-publish test | r3, r5 |
| Q2 | `RDHUP` taken, not asked for | survived: equivalent (Linux does not report it) | - | - |
| Q3 | only `POLLHUP` taken | survived: equivalent here (`ERR` comes only with `HUP`) | - | - |
| Q4 | `POLLIN` asked and taken (readable means gone) | KILLED | the half-close test (99 of 100 in repeats: T2) | r1, r2 |
| Q5 | looked at before the request is read | KILLED | `a_request_whose_client_hung_up_is_not_handled`, through its refusal case | - |
| Q6 | a failing `poll` taken as gone | survived: no test can make `poll` fail | - | - |
| Q7 | a dropped request counted as answered | KILLED | the queued-publish test | r5 |
| Q8 | a dropped request answered `refused ended` | KILLED | `a_request_whose_client_hung_up_is_not_handled` | - |
| Q9 | the poll waits 50 ms | survived: latency only | - | - |
| T1 | timing probe: the count made 300 ms after the answer | "KILLED", which shows T1's race: no behaviour changed | the queued-publish test, at `:504` | r5 passes |

P1 to P5 are the author's plants, in the forms the description gives. Q1 to Q9 and the T1 probe are mine.

**My tests** (`review_tests.rs`, put inside `mod socket` with `insert_tests.py`; not for the pull request unless the author wants them):
- `r1_hung_up_on_real_sockets` and `r1b_a_closed_client_is_gone`: `hung_up` itself, in nine cases and in two. Each case has acted before the poll, so they are deterministic.
- `r2_a_half_closed_client_queued_is_answered`: the half-close before the accept. It is the deterministic form of the PR's half-close test (T2).
- `r3_a_withdraw_and_a_refusal_whose_clients_gave_up`: the other request with an effect, and the refusal's count.
- `r4_a_client_that_gives_up_while_handled_is_still_handled`: pins the boundary the docs state.
- `r5_the_queued_publish_test_with_the_posts_awaited`: T1's fix.
- Each passed 50 of 50 runs (r5: 200 of 200) beside 4 busy loops on 4 CPUs (`repeat_mine.out`, `repeat_r5.out`).

### How I worked

- I read `git diff origin/zfs-topology...pr166` (one commit on 9e70246, `bins/skyzfsd` only) and formed the view above from the code first. I then read my F1 of #149 (`review/m2.6c3/fifth_look.md`), and the description last. It agrees with the code; D1 is its only miss.
- I checked every client of the socket in the tree for how it ends a request: `skyzfs_control::ask` and its users (the agent, `skyzfsd ask`), and `publish_agent.py raw`.
- The gates ran one cargo command at a time in a clean worktree of the head, after `rustup update stable` (`gates.sh`, `gates.out`, `gate_counts.txt`).
- Repeat runs used the built test binary (`repeat.sh`), so that each run times the test and not cargo.
- CI: all nine workflow runs on 7d98b26 are green.
  - The daemon rig's three summary tables were read from its artifacts: 87 rows each, none different from `EXPECTED`.
  - The publish rig's `pchannel` lines were read from its job log.
  - The replay rig's daemon jobs (pull-request budget) caught both controls on both images.
- Line numbers are at 7d98b26.

**Files in this folder:**
- `poll_probe.py`, `poll_probe.out`: the `poll(2)` probe (1).
- `review_tests.rs`, `insert_tests.py`: my scratch tests and how they were put in.
- `plants166.py`, `plants.out`: the plants. `plants_r5.out`: the plants that bear on r5.
- `repeat.sh`, `flaky_head.out`: T1's repeats. `repeat_p2.out`: P2 and Q4 against the half-close tests. `repeat_mine.out` and `repeat_r5.out`: my tests' repeats.
- `baseline.out`: the unplanted control tests, with mine (before r5 was added).
- `gates.sh`, `gates.out`, `gate_counts.txt`: the gates.
