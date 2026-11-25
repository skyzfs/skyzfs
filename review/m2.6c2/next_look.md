# Next look at #147 (M2.6c-2) at 0f709ae

The review was at `366b217` (`review.md`). This look is at **`0f709ae`**,
the pull request's head when it began and when it ended. Every part
below was judged at `0f709ae` except where a line says otherwise:

- the merge with the current base, `8f0390f`, which now holds #146;
- the recorder run, which ran the head's Lua from the branch.

I formed my view from the code before reading the author's answer. I
then checked each claim in the answer against the code.

## Verdict

**No finding blocks the merge.** B1 and B2 are fixed and the fixes hold:

- In a sandbox of the channel programs' globals, the head's program runs
  to its end and the old one fails on `pcall`.
- On ZFS (recorder run 45665823917, ubuntu-26.04, with the head's Lua),
  values with bytes above 0x7f come out as hex, in locale C and in
  C.UTF-8, on both pools.
- The adapter reads those outputs back to the exact bytes that
  `zfs set` was given (a8).

F3, F5, F6, F7, F8 and F9 are done as answered. Their limits are given
below. S1 is done.

**Can it merge?** Yes, once these three things are true, and not before:

1. the recorder's ubuntu-24.04 job on the head is green, as the lead's
   L2 requires, and its `*-snapshot-odd*` records read the same as the
   26.04 ones;
2. `skyzfs-daemon-rig` and `skyzfs-replay-rig` on `0f709ae` are green.
   Both were still running at my last check;
3. every other run on the head is green. At my last check they were.

The findings below can follow in a later pull request. NL-F1 is the one
I would fix first: it is small, and the answer's argument for leaving it
does not hold.

## Findings, most severe first

### NL-F1 (Low, certain). `held()` reads the root alone, and no later `hold` refuses a partial hold

**Where:**

- `crates/skyzfs-agent/src/zfs/cli.rs:558-568` (`held`): `zfs holds -H pool@t`, the root only;
- `crates/skyzfs-agent/src/plan.rs:420` and `:671`: `Action::Hold` only `if !pool.held`;
- `crates/skyzfs-agent/src/engine.rs:550` and `:927`: `pool.held` is `zfs.held()`;
- `engine.rs:634-639`: the error of a `hold` is dropped (`Moved::No`).

At the merge with `8f0390f` the same lines are `plan.rs:430/694`,
`engine.rs:553/937/647`.

**Scenario.** Someone runs `zfs release skyzfs-publish pool/child@t` on a
publish that is held. `held()` still reads true because the root has the
tag, so the planner never plans `Hold` again. The fix in `hold()` for
"tag already exists" never runs: it runs only when `hold` is called,
and `hold` is called only when the root lacks the tag.

The publish goes on as held while `child@t` can be destroyed. If it is
destroyed, the members check (`same_snapshot`) gives up at the next
look. So this is not a silent loss, but a hold that the record believes
in and that is not there.

**The answer** ("the next `hold` refuses it; tell me if you want it") is
right only for the opposite partial, where the root lacks the tag. In
that case `hold` fails on every round until the publish ages out, and
the engine drops the error each time.

**Suggestion.** `held()` = `members()` and `tagged_snapshots()`, true only
where every member's `@t` has the tag. `hold()` already has the code.
The test would be the a5 shape (`nl147_cli.rs`) run through `held`.

### NL-F2 (Low, certain; mostly docs). What the group kill covers

**Where:** `crates/skyzfs-agent/src/host.rs:174-188`.

The limit reached while the child is still running calls `abandon`
(`host.rs:160-171`, `245-262`). `abandon` kills the group, then the
child, and counts it.

The other way to reach the limit is different. The child has ended and
a grandchild still holds its pipes. Then `take(...)` returns `TimedOut`
with no group kill and no count.

h1 (`nl147_host.out`): the grandchild was alive after the call. The
reader threads went from 2 to 4, and `STUCK_NOW` stayed at 0. The
grandchild ended only by itself, later.

h2: a grandchild that calls `setsid` leaves the group. Nothing short of
a cgroup catches that, so this part is inherent.

**Why it is Low.** `zfs` and `zpool` do not fork helpers that outlive
them. A held pipe means a helper stuck in the kernel, where `SIGKILL`
does nothing anyway.

**Suggestion.** Either kill the group on that path too (the group id is
the child's pid, and the child has been reaped, so this needs care), or
correct the answer's "the whole group is killed": the module docs
already say only that the ends "are waited for within the limit".

### NL-F3 (Low, certain; a test gap more than a defect). 0x7f is decided in two places, and no test pins that they agree

The program hexes any byte outside `\32-\126`
(`crates/skyzfs-agent/src/zfs/cli/snapshot.lua:34`). The adapter and the
manifest decide with `manifest::plain`, which is `0x20..=0x7e`
(`manifest.rs:305`). Today they agree.

Plant N1 widens `plain` to 0x7f, and the crate's tests stay green. In a
real run every value containing DEL would then be refused as "hex of a
printable value", and its take destroyed. The drifted-Lua plant `del127`
(a7) shows the refusal from the program's side.

**Suggestion.** Add 0x7f, and 0x1f and 0x20 beside it, to the adapter
test's values.

## Suggestions (none blocks)

- **NL-S1. `destroy_taken`** (`cli.rs:276-293`):
  - **The name.** `zfs destroy -r pool@t` takes `%` ranges and `,`
    lists in the part after `@`. `t` comes from the agent's own names
    today. A `valid_name(t)` guard, or a test that names cannot hold `%`
    or `,`, would keep it so.
  - **The message.** `dataset does not exist` maps to `NoSnapshot`, and
    then to "was not taken". If the pool was exported in between, the
    snapshot may well have been taken. "was not found" is closer.
- **NL-S2. A recursive snapshot left behind by `ManifestTooLarge`**
  (`engine.rs:813-817`): `abandon` keeps the snapshot that was just
  taken. Hex doubles a non-ASCII value's size in the manifest, so about
  64 user properties of 8 KiB of binary reach `MANIFEST_MAX` (1 MiB).
  This was already the case before this head; hex makes it nearer. The
  same `destroy -r` as `destroy_taken` would clean it.
- **NL-S3. Instruction cost.** Hex costs about 9.4 Lua instructions a
  byte (`nl147_lua.out`): the scenario `wide` spends 1,234,200 and
  `wide_plain` 5,400 for 16 x 8191 bytes. At `-t 100000000`, that is
  about 10 MB of values, so `MANIFEST_MAX` binds long before the
  instruction limit. Not a problem. S2 (timing on many datasets) stays
  with the recorder, as answered.
- **NL-S4. The doubled prefix.** The message reads
  `zfs program (snapshot) exited 0 and its output cannot be read: zfs
  program (snapshot): ...` (a7, `cli.rs:291` with `:920`).
- **NL-S5. The PR body.** "Readings that still rest on that run" names
  the run at `366b217`, but the fixtures are from that run and the head's
  Lua has now run (45665823917). The body's line "the whole group is
  killed at the limit" needs NL-F2's limit too.
- **NL-S6. The workspace `cargo doc -D warnings` failure is wider than
  `skyzfs-ublk-sys/src/queue.rs:373`.** With `--keep-going` it fails in
  eight crates this pull request does not touch: `skynbd-agent-cli`,
  `skynbd-agent`, `skynbd-engine`, `skynbd-refs`, `skynbdctl`,
  `skyzfs-local`, `skyzfs-ublk-sys` and `skyzfsd` (`nl147_gates.log`).
  The two crates it touches document clean. This is not this pull
  request's to fix.

## Test gaps (not blocking, ordered by worth)

- **NL-T1. The Lua runs nowhere in CI** (R10 to R12 remain open). No crate
  test runs the programs, so P21 (reintroducing `pcall`) passes the
  crate's tests.

  `nl147_sandbox.lua` catches it with no kernel. It is a Lua 5.2
  sandbox with only the channel programs' globals (no `pcall`,
  `xpcall`, `load`, `loadfile`, `dofile`, `print`, `io`, `os`, `debug`,
  or `string.dump`), a model pool, and an `nvlist_print_json` emitter
  in locale C.

  A `lua5.2` job running it on the three programs would close B1's
  class for good. The text check in `cli/tests.rs` is a narrower guard.
- **NL-T2. The test that needs root skips on CI without failing.**
  `a_socket_or_a_daemon_of_another_user_is_refused` returns early when
  not root. CI's runners are not root, so plants P14 (no peer check),
  P15 (no owner check) and N11 (any peer trusted) are caught here, as
  root, and would pass CI. An `#[ignore]` with a root job, as the
  control rig does for the kernel tests, would make the skip visible.
- **NL-T3. N12 survives.** No test name holds `": "`. A dataset
  `tank/x: no such pool` would tell the first `": "` from the last.
  ZFS names may hold `:` and spaces.
- **NL-T4. N10 survives.** Nothing tests a client that is not root
  reaching a root daemon. That is inherent on CI without root, and
  would go with NL-T2's root job.
- **NL-T5. N8 and N9 survive** (the count taken down before the wait,
  and no kill on `try_wait`'s error). Both need a child stuck in the
  kernel, or a `try_wait` that fails. Inherent in unit tests; the code
  reads right.
- **NL-T6. Not a fixture yet: the partial hold with the root tagged.**
  The recordings have the root-tagged and child-tagged holds. A fixture
  of NL-F1's case would come with its fix.

## The points asked

### 1. B1 and B2

- **`snapshot.lua`.** It has no `pcall`. `value_of` (lines 32-40) takes
  `tostring(v)`. A value is returned as it is when `string.find(v,
  "[^\32-\126]")` finds nothing; otherwise it is
  `{encoding="hex", value=gsub(v, ".", "%02x" of string.byte)}`. This
  is applied to system properties whose source is the dataset itself,
  and to user properties, before the take loop (lines 57-68). A
  property that raises now ends the program before any take: the
  `raises` scenario gives `taken 0`.
- **One form per content.** These rest on the scratch tests in
  `nl147_cli.rs` (a1 to a3, a7 and a8; the output is `nl147_cli.out`):
  - Each byte 0 to 255 alone, all 256 together, and the empty value:
    one form each, and `decode(encode)` gives the same (a2).
  - These are `NotCanonical`: upper-case hex, mixed case, hex of a
    printable value, the empty value in hex, a non-printable value as
    a string, UTF-8 as a string, a tab as a string, and keys out of
    order.
  - These are `Shape`: odd length, a sign, a non-ASCII digit, an extra
    key, and the encoding's case (a3).
- **The sandbox's output.** The adapter reads it back to the bytes the
  program was given (a1). The expected bytes come from `string.byte`,
  not from the program's own hex.
- **ZFS's output** (a8, `nl147_zfs/`):
  - records: `rig-snapshot-odd`, `rig-snapshot-odd-utf8` and
    `mirror-snapshot-odd`;
  - the values set: `naïve, 日本, ∅` and `a\xff\xfeb\xc3`;
  - what was read: no destroy, the same bytes, and a manifest that
    round-trips.
- **Limits.** See NL-S3. `-m` is the program's memory limit, and it is
  not reached. `MANIFEST_MAX` is the limit that binds.
- **`destroy_taken`.**
  - It runs `zfs destroy -r pool@t` within `limits.change`.
  - The outcomes are `destroyed`, `was not taken` (from "could not
    find any snapshots to destroy", or `NoSnapshot`) and `could not be
    destroyed: ...`.
  - It is called only on output that cannot be read after exit 0, or on
    an `Other` result without `failed`. A failure the program reported
    itself is not destroyed (P4 caught).
  - A cut output can only come from the success return, which carries
    the locals. The refusals (errno 17, a partial take) carry none and
    are not cut. So `destroy -r` removes what the program took. a4: the
    planted program without hex is cut at byte 172, and `tank@t
    destroyed`.
  - The wording is under NL-S1.
- **Does a mock or sandbox catch P21?** The crate's tests do not (NL-T1).
  The sandbox does: on the `366b217` program it fails with "attempt to
  call global 'pcall' (a nil value)" and `taken 0`, matching what ZFS
  printed (`old-bytes.expected`).

### 2. F3

- `hold` matches "tag already exists" on stderr, after the last `": "`.
  On that message it reads the members and `zfs holds -H -r pool@t`,
  and is `Ok` only where every member's `@t` has the tag.
- a5:
  - root held and child not: an error;
  - child held and root not: an error;
  - all held: Ok;
  - a tag found in the command line is never matched.
- P6 and N7 (the holds' tag field not compared) are caught.
- The tag is `skyzfs-publish` (`HOLD_TAG`).
- What `hold` cannot fix is NL-F1.

### 3. F5, L3 and F6

- **The process group.** `process_group(0)` is at `host.rs:129`. The
  kill is in `abandon`. A stopped child is killed and reaped (h3).
- **The cap.**
  - `STUCK_MAX` is 4 (`host.rs:40`). It is checked at `:119-125`.
  - The count goes down in the reaper after `wait`.
  - Five calls past their limit in a row, one after another, are not
    refused. The count is back at 0 after each (h4), so the cap counts
    children that are still running, not timeouts.
  - The cap belongs to the process (NL-D1).
- **F6.** `checked_add` is at `host.rs:113` and in the client's
  `deadline()`. Both have tests.
- **The limit of the group kill.** See NL-F2.

### 4. L5: `lstat` and `SO_PEERCRED`

The checks are in `crates/skyzfs-control/src/client.rs`:
`trusted` at 97-99, `check_path` at 103-126 and `check_peer` at
130-143. The order is `lstat`, then `connect`, then the peer check.

- **What is refused before any try** (c1): a directory, a regular file
  and a FIFO at the path, each `PermissionDenied` with 0 tries. A link
  at the path is refused too (P13 caught).
- **The race between the `lstat` and the `connect`** (c2): a link to
  another user's listener is swapped in.
  - 3,006 asks reached that listener, and the peer check refused every
    one.
  - None was accepted.
- **What the peer check cannot tell apart:**
  - c3: through the same race, a link to another root listener is
    accepted (1,294 times).
  - c4: a listener made by root that then drops to uid 54321 is still
    root to `SO_PEERCRED`, which gives the credentials of `listen()`.
  - Both need root, so the trust boundary is root, as intended.
- **The other user's socket**: refused by owner (P15) and by peer (P14),
  as root only (NL-T2).
- **On CI** the test that needs root returns early. It does not fail.
- **The rule's sides.** It is the same in form as the daemon's
  `same_user_or_root` (`bins/skyzfsd/src/control/server.rs:71`), but
  each side applies it to the other. See NL-D2.

### 5. L1 to L7 as ruled, and the rest

The lead's rulings are not in the pull request's thread. I judged them
by the author's account of them.

| Item | Status at `0f709ae` |
|---|---|
| L1 | Done: hex in the program, bytes in the manifest |
| L2 | The answer agrees not to merge before the kernel run. It is half in (26.04 green, 24.04 running) |
| L3 | Done (NL-D1) |
| L4 | Done: any `remove:` line gives `Refusal::Removed`, and the fixture ZFS never prints is gone |
| L5 | Done (NL-D2) |
| L6 | Received values stay out of the manifest |
| L7 | The limits are kept and marked provisional |
| F4 | The docs are fixed, and the recordings show `zfs program` saying `no such pool` |
| F7 | As L4 |
| F8 | As L5 |
| F9 | The docs list each partial take and its outcome |
| S1 | `says()` (`cli.rs:600-605`). P20 is caught; N12 survives (NL-T3) |
| S2 | Deferred to the recorder |

`zpool get` saying `missing pool name.` (`cli.rs:437-449`):

- Only an `Other` error with that message leads to a look with
  `zpool status -P`, and only that look's `NoPool` gives `NoPool`.
- Any other result of the look returns the original error. a6: the
  status was killed at its limit, and the result was not `NoPool`.
- N6 (`NoPool` with no look) is caught by two recordings tests.

Test gaps R1 to R3, R9 and R14 are pinned by the new tests. Plants P10,
P11 and P12 are caught, and R3, R9 and R14 are covered as answered.

### 6. Gates, plants, the diff, CI

**Gates at `0f709ae`** (`nl147_gates.sh`, `nl147_gates.log`). These
were run from a clean checkout, with rustc 1.100.0 (current stable,
checked with `rustup update stable`):

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -D warnings`: plain, with `test-hooks`, and with `control-test-program` | clean |
| `cargo test -p skyzfs-agent -p skyzfs-control` (run as root, so the test that needs root ran) | 114 + 16 passed |
| `cargo doc -D warnings` for the two crates | clean |
| `cargo doc -D warnings` for the workspace | fails in untouched crates (NL-S6) |

**The merge with `8f0390f`** (`nl147_merge.log`): `git merge-tree` gives
no conflict. Clippy `-D warnings` on the agent and the control crate is
clean. Their tests pass, 119 + 16.

**Plants** (`nl147_plants.py`, `nl147_plants.out`). Each plant was
applied alone, followed by `cargo test -p CRATE --lib` as root.

| Plant | What it plants | Result |
|---|---|---|
| P2 | A result of the wrong shape not destroyed (author's) | caught |
| P4 | A reported failure destroyed too (author's) | caught |
| P6 | A member without the tag missed (author's) | caught |
| P10 | No process group of its own (author's) | caught |
| P11 | The count never taken down (author's) | caught |
| P12 | The cap off by one (author's) | caught |
| P13 | The path read through a link (author's) | caught |
| P14 | The peer not read (author's) | caught as root only |
| P15 | The owner not read (author's) | caught as root only |
| P19 | A sign taken as hex (author's) | caught |
| P20 | Messages matched anywhere (author's) | caught |
| P21 | `pcall` reintroduced | not caught by the crate; caught by the sandbox |
| N1 | 0x7f printable | survives (NL-F3) |
| N2 | Hex of any case | caught |
| N3 | Hex of a printable value | caught |
| N4 | Destroy without `-r` | caught |
| N5 | Any destroy failure read as "not taken" | caught |
| N6 | `missing pool name` read as `NoPool` with no look | caught |
| N7 | The holds' tag field not compared | caught |
| N8 | The count taken down before the wait | survives (NL-T5) |
| N9 | No kill on `try_wait`'s error | survives (NL-T5) |
| N10 | Root not trusted | survives (NL-T4) |
| N11 | Any peer trusted | caught as root only |
| N12 | The first `": "` | survives (NL-T3) |

The drifted-Lua plants (upper-case hex, DEL as a string, hex of a space;
`nl147_lua_out/snapshot-*.lua`) are refused by the adapter, and their
take is destroyed (a7).

**The diff `366b217..0f709ae`** is one commit, 19 files. Everything in it
is in the answer's list:

- the fixtures and `recordings.rs`;
- `Layout.removed` and `Refusal::Removed`;
- `rustix` added to the agent for `kill_process_group`;
- the change of `local` to bytes in `engine/fake.rs`;
- the new tests.

**Were tests weakened?** No. The removed assertions are of three kinds:

- for the `pcall` branch, which is gone;
- for the `indirect-3` fixture, which ZFS never prints;
- for a plain file at the socket's path, which is now refused earlier
  as `PermissionDenied`.

**Manifest v1 is amended in place** (NL-D3).

**CI on `0f709ae`.** Read with `gh api .../actions/runs?head_sha=...`:

| Run | Result |
|---|---|
| ci (`unit-arm64` included) | success |
| skyzfs-ci | success |
| coverage | success |
| e2e | success |
| k3s-smoke | success |
| skyzfs-nodeb-rig | success |
| skyzfs-control-rig | success |
| skyzfs-publish-rig | success |
| skyzfs-cut-daemon | success |
| skyzfs-daemon-rig (45665358670) | in progress |
| skyzfs-replay-rig (45665358640) | in progress |
| Recorder 45665823917 (testing `0b394b2`, `lua_ref` = `skyzfs-coding-m2.6c-2` = `0f709ae`; its Lua files are byte for byte the head's) | unit, lua and ubuntu-26.04 success; ubuntu-24.04 in progress |

On 26.04 all three programs exited 0 on both pools. The errno-17
refusal and `absent` were seen. The `none-*` records exited 1, with no
pool, as expected.

## Decisions for the lead

- **NL-D1 (L3). The cap belongs to one process.** `STUCK_NOW` is a static,
  so it starts at 0 in each agent process. The cap bounds a long-running
  agent. A per-command process (M2.6c-3) or a scheduled run (M2.7)
  starts from 0 each time, while the children of the last run may still
  be stuck. If M2.7 runs the agent as one command per tick, the cap does
  not bound anything across ticks. You may want the count kept outside
  the process, or a look at the stuck `zfs` processes, before M2.7.
- **NL-D2 (L5). One rule, two sides.**
  - The client trusts a peer that is root or its own effective user
    (`client.rs:97-99`). The daemon accepts a client that is root or its
    own user.
  - With a root agent and a daemon under a service account, the daemon
    accepts the agent, but the agent refuses the daemon.
  - Fine if both always run as root. Say so if a service account is
    planned.
  - The race between the `lstat` and the `connect` is closed by the peer
    check, except against other root listeners (c3).
  - Connecting through an `O_PATH` descriptor of the checked inode would
    close it entirely. That is optional.
- **NL-D3. Manifest v1 changed in place.**
  - `Member.local` is now bytes, with a hex object for values that are
    not printable.
  - A v1 manifest of the old code with a tab or UTF-8 in a value is
    refused now (`NotCanonical`).
  - No real adapter on `zfs-topology` has written one (the CLI adapter
    is new in this pull request), and nothing else in the tree reads
    manifests.
  - So amending v1 rather than writing v2 is safe. It still needs your
    agreement under the versioned-formats rule.
- **NL-F1's fix.** The author asked whether to widen `held()`. My answer
  is yes, in this pull request or the next.

## What this look does not cover

- **24.04 with the head's Lua.** The recorder's ubuntu-24.04 job had not
  finished, so the head's programs on 24.04 are unread. Neither the
  daemon rig nor the replay rig had a result on the head.
- **The root-only test on CI.** I ran it as root here. CI skips it
  (NL-T2).
- **NL-F1 on ZFS.** I did not run its scenario on ZFS. It rests on the
  code path, which is certain.
- **The workspace's tests.** I ran the agent and control crates only;
  `ci` and `skyzfs-ci` ran the workspace green.

## Files (beside this note)

| File | What it holds |
|---|---|
| `nl147_sandbox.lua` | The sandbox, the model pool and the C-locale JSON emitter. Run with `lua5.2` |
| `nl147_lua.out` | The sandbox's summary |
| `nl147_lua_out/` | The programs, the plants, their JSON and the expected bytes |
| `nl147_cli.rs`, `nl147_cli.out` | Adapter tests a1 to a8 |
| `nl147_zfs/` | The recorder's records used by a8 |
| `nl147_host.rs`, `nl147_host.out` | Runner tests h1 to h4 |
| `nl147_client.rs`, `nl147_client.out` | Client tests c1 to c4, run as root |
| `nl147_plants.py`, `nl147_plants.out` | The plants and their results |
| `nl147_gates.sh`, `nl147_gates.log` | The gates at the head |
| `nl147_merge.log` | The merge with `8f0390f` |
