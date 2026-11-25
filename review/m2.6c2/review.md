# Review of #147 (M2.6c-2): the agent's adapters, and one deadline per ask

**Head judged: `366b217`** (`skyzfs-coding-m2.6c-2`), on `8663cd7`. The head
did not move during the look (read at its start and its end). The base has
since moved to `a4e004e` (#145, the rigs' Python and the notes, no crate
code); the branch still merges with it without a conflict.

**Verdict: not ready to merge.** Two blocking defects, both in
`snapshot.lua`, both certain and both small to fix:

- **B1.** The program calls `pcall`, which ZFS channel programs do not have.
  Every snapshot program fails before it takes anything. No publish can
  ever succeed through `ZfsCli`.
- **B2.** A locally set property with a byte above 0x7f makes
  `zfs program -j` print cut-off JSON and still exit 0, after the
  snapshots are taken. Every publish of such a pool is lost and leaves a
  recursive snapshot behind.

The runner, the release and held readings, and S4 hold up. I tested them
with scratch tests and plants. What remains below is Low, doc, or test
gaps. The decisions for the lead are listed at the end.

Files beside this note:

| File | What it holds |
|---|---|
| `gates147.sh`, `gates147.log` | The gates |
| `scratch147_host.rs`, `scratch147_host.out` | The runner's scratch tests, r1 to r8 |
| `scratch147_client.rs`, `scratch147_client.out` | The client's scratch tests, k1 to k5 |
| `plants147.py`, `plants147.out` | The plants, one JSON line each |
| `mbrtowc147.c`, `mbrtowc147.out` | What glibc's C locale does with a UTF-8 byte |

The scratch tests were planted in a worktree at the head and then
removed.

---

## Blocking

### B1. `snapshot.lua:42` calls `pcall`, which channel programs do not have (certain)

`local ok, value, source = pcall(zfs.get_prop, ds, property)`

**Evidence.**
- OpenZFS `module/lua/lbaselib.c` (read at `zfs-2.2.2`; master has the
  same comment) opens with "The following built-in lua functions have
  been removed and are not available for use in ZFS channel programs:
  dofile loadfile load pcall print xpcall". Its `base_funcs` has no
  `pcall`.
- zfs-program(8) says the same: "The following functions base library
  functions have been disabled and are not available for use in channel
  programs: dofile loadfile load pcall print xpcall".

**What happens.**
- `zfs.list.system_properties(ds)` gives every visible property valid
  for the dataset's type (`zcp_iter.c`, `zcp_dataset_system_props`). The
  nvlist is all booleans, so it becomes a Lua string array, and the list
  is never empty. So the loop body always runs.
- Its first line raises "attempt to call global 'pcall' (a nil value)",
  before any `zfs.sync.snapshot`, and `zfs program` exits non-zero.
- `failure()` reads this as `Other`. The engine's `take` gives
  `Moved::No`. At the next round the phase is still `Intent` with no
  `pool@t`, so it takes again. This repeats until `AGE_MAX`, and then
  the publish is abandoned as too old.
- It is safe, in that nothing is taken. But the adapter can never
  publish.

**Why nothing caught it.** The author's mock ran the programs in
standard Lua 5.2, which has `pcall`. The one Rust test of the program
reads its text.

**Fix.** Drop the `pcall`:
`local value, source = zfs.get_prop(ds, property)`.
- A property that raises (`zcp_get.c`, `zcp_handle_error`: EINVAL, EIO,
  anything but ENOENT) then fails the whole program before any take. That
  is what the module docs promise. ENOENT gives no value and does not
  raise.
- The `failed`/`property`/`why` branch of `parse_program` then cannot
  happen. Remove it, or keep it for an older program.
- My reading of `get_special_prop` and `get_zap_prop` finds no visible
  property that raises on a healthy dataset. Whether one does is still
  for Q10 to show on a real pool. If one does, read a list of the
  properties the manifest needs instead of all of them. That also
  shortens the time the program holds the sync (S2 below).

### B2. A byte above 0x7f in a local property's value cuts the program's JSON after the take (certain mechanism; the trigger is any such value)

**Evidence.**
- `lib/libnvpair/libnvpair_json.c`, `nvlist_print_json_string`, decodes
  each string with `mbrtowc`. On `(size_t)-1` or `-2` it returns -1 in
  the middle of the string: the opening quote is printed and nothing
  after it.
- `cmd/zfs/zfs_main.c`, `zfs_do_channel_program`, calls
  `(void) nvlist_print_json(stdout, outnvl);`. The error is ignored, and
  the command exits 0.
- The runner sets `LC_ALL=C` (`host.rs:88`), and `zfs` calls
  `setlocale(LC_ALL, "")`.
- On glibc 2.39 (Ubuntu 24.04) in that locale, `mbrtowc` gives -1 on
  `0xc3`, the first byte of "é" (`mbrtowc147.out`).
- Property names and dataset names are ASCII by ZFS's name rules. So only
  values matter: any user property, or a string system property set
  locally (`mountpoint`, `sharenfs`, `sharesmb`, `keylocation`, ...).

**What happens.**
1. The program takes every member's `@t`.
2. `zfs program` prints `{"return":{... "local":{"mountpoint":"/srv/caf`
   and exits 0.
3. `returned()` gives "not JSON", which is `Other`, and the engine's
   `take` gives `Moved::No`.
4. At the next round `pool@t` exists at `Intent`. The planner gives up
   with `ManifestLost`: the snapshot is kept, unpublished, without a
   hold.
5. The next publish takes a new recursive snapshot and fails the same
   way.

So such a pool never publishes, and each attempt leaves a recursive
snapshot behind, without bound.

**A UTF-8 locale does not fix it.** Invalid UTF-8 still cuts the output.
Worse, characters above U+FFFF are dropped without a word, so the
manifest silently differs from the pool. Plant R15 shows the locale is
pinned by a test, as it should be.

**Fix** (the form is decision L1).
- Encode each value in the program before it is returned, for example as
  hex: `(value:gsub(".", function(c) return string.format("%02x", c:byte()) end))`.
  The string library is available in channel programs. Decode it in
  `parse_program`.
- Then decide what the manifest does with bytes that are not UTF-8:
  `manifest::Member.local` is `String` to `String`.
  - If such a value is refused, refuse it in the program, before the
    take. A refusal in Rust comes after the take and leaks the snapshot
    as above.
- The least that should hold for the merge: no value can make the
  program's output unreadable after the take.

---

## Not blocking

### F3 (Low). `hold`: "tag already exists" is read as held for every member, and is matched on the command line too

`cli.rs:330`: `Err(ZfsError::Other(why)) if why.contains("tag already exists") => Ok(())`.

**The text it matches.** `why` is `"{command}: exit N: {stderr}"`, and the
command words hold the pool's name and the tag. Match `stderr` alone.

**What the message means.** It says only that some member has the tag.
- `dsl_userhold.c`, `dsl_dataset_user_hold_check`, fails the whole sync
  task on any error but ENOENT. So EEXIST on one member means nothing
  was held, not even on the members that lack the tag.
- `held()` reads the root alone (`cli.rs:369-379`).

**How the two meet.** Suppose a person releases the tag on one member
without `-r`.
- **The root has the tag and a child lacks it:** `held()` is true, and
  the publish goes on with the child's `@t` not held.
- **The root lacks the tag and a child has it:** `held()` is false, so
  the planner chooses `Hold`, the hold returns Ok, and `Moved::Yes`
  means the next round starts at once, with no `TICK`. The result is a
  tight loop of about seven commands a round until `AGE_MAX`.

**Suggestion.** On "tag already exists", read `zfs holds -H -r pool@t`,
as `release` does. Return Ok only if every member that has `@t` keeps the
tag; anything else is `Other`. `held()` could require the same.

**Certainty.** High on what ZFS does. The scenario needs a person's
partial release, or the libzfs edge in F4's last bullet.

### F4 (Low, docs). `zfs` commands never say "no such pool"

**What happens.** A pool that is exported or gone gives
`cannot open 'pool@t': dataset does not exist`. That is libzfs
`zfs_standard_error`, ENOENT with its aux message. So:
- `hold`, `release` and `holds` read it as `NoSnapshot`;
- `release` gives Ok, and the engine commits `held = false`;
- `held` gives false;
- `zfs program` on a missing pool gives "Channel program execution
  failed: dataset does not exist", which is `Other`.

Only `zpool` commands say "no such pool". The module docs (`cli.rs:29-33`)
and the PR body's "Errors" say otherwise.

**Why it is not unsafe.** Nothing is released on disk. The engine reads
`zpool get` first (`NoPool` means no round), so only an export racing a
round reaches this.

**Suggestion.** Fix the doc. If wanted, re-read the pool with
`zpool get` before believing `NoSnapshot` in `release`.

**Another way release reads Ok.** libzfs `zfs_release_one` puts a member
whose `lzc_get_holds` fails for any reason (EIO included) down as ENOENT,
and skips it. So `release -r` can exit 0, or give "dataset does not
exist", while that member keeps the tag. The tag is left in place (a
leak, the safe direction), and only a failing read of a member leads
there.

### F5 (Low). The runner: what a call past its limit leaves

**What I measured** (`scratch147_host.out`):
- The child is killed and reaped when it ignores TERM, INT and HUP (r2),
  and when it is stopped (r3). It does not reach its next step, and no
  zombie stays (r1, r5).
- The call returns at the limit, about 300.2 ms in each case.
- The writer of a 1 MiB input to a child that never reads ends (r7).
- A child of the child that holds the pipes is not killed. There is no
  process group, so it outlives the call and keeps the two reading
  threads:
  - r4: 2 threads before, 4 after, back to 2 when it ends;
  - r5: 8 calls gave 18 threads, back to 2 after 4.8 s.

**Gaps against the docs and code.**
- A child in D state also keeps its reaping thread, so three threads,
  not the two that `host.rs:15` and the PR body say. That becomes four if
  its input was not consumed (more than a pipe's 64 KiB; the programs are
  about 2 KiB).
- `host.rs:112`: an error of `try_wait` returns with the child neither
  killed nor waited for.

**How far it can grow.** It is bounded in practice:
- `channel_program`, `hold`, `release`, `get_holds`, dataset reads and
  `zfs set` all check for a suspended pool (`zfs_ioctl.c`,
  `POOL_CHECK_SUSPENDED`). On a pool already suspended they fail at once
  with EAGAIN. Only a command already in flight when the pool suspends
  waits in the kernel.
- The agent runs as one program per command (M2.6c-3), so the threads go
  with it.
- `zfs` and `zpool` do not fork, so the grandchild case is theoretical
  for them.

**Suggestion.** Correct the doc. Optionally kill on a `try_wait` error,
and start the child in its own process group
(`CommandExt::process_group(0)`, then `killpg`).

### F6 (Low). `Instant::now() + limit` panics on a huge limit

At `host.rs:85`, `client.rs:20` and `client.rs:133`. Callers pass
constants today. `checked_add` would turn a caller's mistake into an
error. Suggestion.

### F7 (Low; decision L4). The premise's "no indirect vdev" cannot be read from `zpool status`

`cmd/zpool/zpool_main.c`, `print_status_config`, returns at once for
`VDEV_TYPE_INDIRECT`, and the class printer skips such vdevs too. So
`zpool status` never shows `indirect-N`.
- A pool whose other top-level vdev was removed passes the premise,
  though `zfs.rs:16-17` says it is refused ("no vdev was removed (no
  indirect vdev: conservative)").
- The fixture with `indirect-3` (`cli/tests.rs:61`) shows a tree ZFS
  does not print.
- As data this is likely harmless: a removed vdev's blocks are copied to
  the vdevs that remain, which here is the volume.
- A finished removal leaves a `remove:` line ("Removal of vdev N copied
  ... completed on ..."). `parse_status` reads it and ignores it unless
  it says `in progress`.

**Certain** for 2.2's source.

### F8 (Low; decision L5). The client follows a link at the socket's path and does not ask who answers

k4: an ask through a symbolic link to another socket reaches it and
believes its answer. The agent runs as root and acts on "done": it
releases its hold and forgets the publish. If the volume's run directory
is root's alone this is moot. Otherwise, a peer check in the client
(`SO_PEERCRED`: uid 0, or the daemon's) is cheap defence.

### F9 (Low, docs). A partial take can happen, and "takes nothing" is overstated

Each of these leaves some or all members' `@t` taken:
- **ENOSPC part way through.** `zcp_synctask.c`, `zcp_synctask_wrapper`,
  adds about 144 KiB to `zri_space_used` for each successful check and
  each take. On a nearly full pool the k-th take fails after every check
  passed.
- **EDQUOT.** `snapshot_limit` on an ancestor counts each member's take
  as it is made. Each check sees only its own.
- **EIO from a take.** It raises (`zcp_sync_task`), after the takes
  before it. There is no rollback.
- **The instruction limit.** Hit in the take loop or in the GUID reads
  after it, it gives "Timed out" after some or all takes.
- **The return value.** One larger than `-m` (100 MiB) gives "Return
  value too large" after all takes.

All of these are `Other`, and the next round's `ManifestLost` makes them
safe; the snapshots stay. The PR body's "A pool with more datasets than
those allow fails the program, which takes nothing", and the same in
`cli.rs:64`, hold only when the limit is reached before the take loop.
Fix the doc.

### S1 (suggestion). Match messages on `stderr`, and on the part after the name

`failure()` (`cli.rs:409`) looks for "no such pool" anywhere in stderr,
and stderr names datasets. A dataset may contain spaces, so a child named
`no such pool` would make every failure of a command on it read as
`NoPool`. This is absurd as a case, and nearly free to close: match the
message after the last `": "`.

### S2 (suggestion, for Q10). Time the snapshot program on a pool with many datasets

Without `-n`, the whole program runs in syncing context: one
`zfs.get_prop` call per visible system property of each dataset (several
dozen), plus each one's user properties. For
that time the pool's transaction group cannot finish, and writers wait.
That is negligible on the rigs' pool. Q10 could time a pool of, say,
1000 file systems, so the lead knows the cost of reading the properties
in the same group.

---

## The readings, judged against OpenZFS

| Reading | Verdict | Source |
|---|---|---|
| `zfs program` reads the script `-` from stdin; `-j` prints `{"return": ...}` | certain | `zfs_main.c`, `zcp.c` |
| `zfs.list.user_properties` (with `properties` as its old name) gives (name, value, source); `system_properties` gives a list | certain | `zcp_iter.c` |
| A value's source is the member's own name when set locally, `"$recvd"` for a received one, an ancestor's name when inherited, `"default"`, or nil for read-only ones | certain | `dsl_prop.c`, `zcp_get.c` `get_prop_src` |
| Received values are not "local" here | a choice | Is it the design's? (L6) |
| `zfs.check.snapshot` gives 17 for an `@t` that exists, before anything changes | certain | |
| A take's error other than EIO comes back as a number; EIO raises | certain | `zcp_sync_task` |
| `hold -r` is atomic except for members with no `@t` | certain | |
| `release -r` releases what has the tag; with none, the error is the last member's (ESRCH "no such tag on this dataset", or ENOENT) | certain | `libzfs_dataset.c`, `dsl_userhold.c` |
| `zfs holds -H` prints name, tag and time, tab-separated | certain | |
| A scan line saying `resilver ... in progress` or `rebuild` is busy; a scrub is not | certain | `zpool_main.c`: "resilver in progress since", and for a rebuild "resilver (%s) in progress since"; "scrub" and "error scrub in progress" are not busy |
| Checkpoint `-` or `0` is none with `-p` | likely | The rigs will show it |
| The program limits are the module's maxima (`zfs_lua_max_instrlim` 100 000 000, `zfs_lua_max_memlimit` 100 MiB) | certain | |
| `indirect-N` in `zpool status` | wrong | F7 |
| `no such pool` from `zfs` | wrong | F4 |
| `pcall` | wrong | B1 |
| JSON of non-ASCII values | wrong | B2 |

---

## The points asked

1. **`host.rs` promises.** Scratch tests r1 to r8, and plants R1 and R2.
   - **Kept:** the kill, the reaping, a child that ignores TERM, a
     stopped child, the input's writer, the output bound, no zombies, and
     the return at the limit.
   - **Not kept:** a grandchild is not killed, and a D-state child holds
     three threads, not two (F5).
   - **Not pinned by the PR's tests:** the kill itself and the reaping.
     R1 drops the child unkilled and R2 never reaps it; both survive the
     crate's tests, and r1 and r5 catch them.
2. **`snapshot.lua` and the other two programs.**
   - B1 and B2 above.
   - Partial takes are reachable, and each maps to `Other`, then
     `ManifestLost` (F9).
   - EEXIST with no `taken` is `Exists`. That is right: the check stage
     has changed nothing.
   - There is no injection. Every name reaches the program as data in
     `argv`, the program's text is fixed, `string.find` is plain, and the
     commands get argv, not a shell.
   - `members.lua` and `tagged.lua` are read-only (`-n`), and their
     tables are keyed by ASCII names. They have no B2 exposure.
   - No Lua logic runs in CI. Plants R10, R11 and R12 change what the
     programs return and survive every test (see below).
3. **The release and held readings.** Nothing is read as released while a
   member keeps the tag. The exception is the libzfs edge in F4: a member
   whose holds cannot be read. That leaks a hold, the safe direction.
   Nothing gives up a hold: `release` is called only where the planner
   chose it, and `held()` false leads to `Hold`, not to a release. Every
   unknown message is `Other`. The one weak reading is `hold`'s (F3).
4. **S4 and `ASK_LIMIT`.**
   - **The deadline holds.** k1 (a partial line, then nothing) ended at
     423 ms against a 400 ms limit. k2 (a 1 MiB manifest the daemon never
     reads) ended at 429 ms. k5 (a full backlog for 0.8 of the limit,
     then an answer a byte every 100 ms) ended at 1.0017 s against 1 s.
   - **Not pinned by the PR's tests:** that the connect's wait counts
     (R3 moves the deadline after the connect and survives; k5 catches
     it), and that a timed-out read maps `WouldBlock` to `TimedOut` (R14
     survives the PR's tests; k1 catches it).
   - **The reply is bounded.** k3: more than 4096 bytes with no newline
     is `InvalidData`.
   - **skyzfsd's CLI** (`main.rs:66`, `commands::ask`) now waits 40 s in
     all. The daemon's own bounds are `REQUEST_LIMIT` 10 s and
     `ANSWER_LIMIT` 30 s, so a daemon that answers in its time answers
     within it. The rigs call the CLI under 120 s wrappers.
   - **"30 s start bound" and "exit 9"** (`Exit::NotStarted`) are in
     `serve`, which does not use the client: no change.
   - **`HOLD_MARGIN = ASK_LIMIT`** still covers one whole ask, now more
     tightly.
   - **The socket path:** F8.
5. **Anything else.**
   - No `unwrap` or `expect` outside tests. The two casts (`stem[5..]`
     after `starts_with("raidz")`, and `n as u64` for a signed GUID) are
     sound.
   - No new dependency: `Cargo.toml` and `Cargo.lock` are unchanged.
   - Docs that overstate: F4, F5, F9, and the PR body's "the Lua
     programs ... behaved as intended" (B1).
6. **Gates and CI:** below.

---

## Plants (`plants147.out`)

Each plant was applied alone. The crate's own tests ran with
`--skip scratch147`; where named, the scratch tests ran after them.

**The author's sample, all caught by the test the PR names:**

| Id | Plant | Caught by |
|---|---|---|
| C1 | A timeout per read | `an_ask_ends_at_its_limit_however_the_daemon_spreads_its_answer` |
| H1 | Output waited for after the kill | `a_program_past_its_limit_is_killed_and_not_waited_for` |
| H2 | No limit | `a_program_past_its_limit_is_killed_and_not_waited_for` |
| H3 | Output read with no limit | `an_output_held_open_past_the_end_is_waited_for_only_to_the_limit` |
| Z8 | EEXIST after a partial take read as `Exists` | `the_snapshot_programs_failures_say_what_was_taken` |
| Z9 | No such tag, released with no look | `a_release_without_the_tag_is_released_where_no_member_keeps_it` |
| Z12 | "Tag already exists" an error | `a_hold_already_there_is_held` |
| Z19 | The snapshot program run read-only | `the_snapshot_program_makes_the_manifests_members`, `the_snapshot_programs_failures_say_what_was_taken` |
| D1 | Any node a device | `the_device_is_the_block_devices_number` |

**The review's own:**

| Id | Plant | Crate's tests | Scratch |
|---|---|---|---|
| R1 | The child dropped, not killed or reaped | **survives** | r1 catches |
| R2 | Killed, never reaped | **survives** | r1, r2, r3 and r5 catch |
| R3 | The deadline starts after the connect | **survives** | k5 catches |
| R4 | `release`'s look reads the wrong field | caught | |
| R5 | `parse_program` does not require the root | caught | |
| R6 | `parse_members` does not require the root | caught | |
| R7 | `held` does not compare the snapshot's name | survives | (harmless: `zfs holds` names one snapshot) |
| R8 | `release`: `holds -r` with no snapshot is an error | survives | (the safe direction) |
| R9 | `DaemonSocket` ignores its limit | **survives** | |
| R10 | `snapshot.lua` takes inherited and default system properties as local | **survives** | |
| R11 | `snapshot.lua` takes inherited user properties as local | **survives** | |
| R12 | `members.lua` never says `absent` | **survives** | |
| R13 | "dataset does not exist" is `NoSnapshot` for every command | survives | (no effect on the engine) |
| R14 | A socket's timeout is given as `WouldBlock` | **survives** | k1 catches |
| R15 | The locale `C.UTF-8` | caught | |

## Test gaps (not blocking, ordered by worth)

1. **The Lua programs run nowhere in CI** (R10 to R12, and B1). The
   mock that found nothing is not in the tree. Committing it would help:
   a `lua5.2` job, or a Rust test that runs the programs against the
   mock. Better, with the `pcall` fix, would be a first kernel job (Q10)
   that runs the three programs on a rig pool before M2.6c-3 relies on
   them. The mock must not have `pcall`, `xpcall`, `load`, `loadfile`,
   `dofile` or `print`.
2. **The kill and the reaping** (R1, R2): r1 and r5 would pin them.
3. **The connect inside the one deadline** (R3) and **the `WouldBlock`
   mapping** (R14): k5 and k1 would pin them.
4. **`DaemonSocket`'s limit** (R9): an ask of a listener that never
   answers, with a 100 ms limit, ending near 100 ms.

---

## Gates (`gates147.log`), at `366b217`, stable 1.100.0

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings`, plain | clean |
| the same, with `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean |
| the same, with `skyzfs-ublk-sys/control-test-program` | clean |
| `cargo test -p skyzfs-agent -p skyzfs-control` | 97 and 11 passed, 0 failed |
| `RUSTDOCFLAGS=-D warnings cargo doc --no-deps -p skyzfs-agent -p skyzfs-control` | clean |

`cargo test -p skyzfsd`, plain and with `test-hooks`, did not build here:
the container's disk is shared and full, and its dependencies did not
fit. CI ran those tests on the head (below).

## CI on the head (`366b217`)

| Run | Result | What it covers |
|---|---|---|
| `ci` | green | lint (fmt, clippy), `test` (the workspace's tests, skyzfsd's included), `unit-arm64` (clippy and tests on arm64, plain and with each feature), s3-backends, chart-lint |
| `skyzfs-ci` (push) | green | gate, lint, test, `unit-arm64` |
| `coverage`, `e2e`, `k3s-smoke` | green | Not the agent |
| `skyzfs-replay-rig` | green | The daemon jobs, "through skyzfsd" and "write into a hole", on 24.04 and 26.04. The reference replay was skipped: the PR does not touch `tests/kernel/` |
| `skyzfs-cut-daemon` | green | 26.04, 26.04-upload, 26.04-arm |
| `skyzfs-publish-rig` | green | 26.04: the rig's own agent drives skyzfsd's CLI, which uses the client with S4 |
| `skyzfs-daemon-rig` | green | 24.04, 26.04 and 26.04-arm, on skyzfsd. I did not read its summary table: the PR changes no daemon code, only the client its CLI uses |

The client's change (S4) runs in these rigs through skyzfsd's CLI. No
run exercises `ZfsCli` or the Lua programs against ZFS. That is by
design: nothing calls them yet, and Q10 is to come. So B1 and B2 are
invisible to every green run.

---

## Decisions for the lead

- **L1 (B2).** What the manifest carries for a property value that is
  not ASCII, or not UTF-8. The options:
  - hex in the program, then bytes in the manifest;
  - hex in the program, then refuse non-UTF-8, which must be done in
    Lua before the take;
  - refuse any non-ASCII value before the take, the least for now.
- **L2 (B1, Q10).** Whether the merge waits for a first kernel run of the
  three programs, or that run is the first thing after it, before
  M2.6c-3. My view: the fixes for B1 and B2 are small and certain, and
  can merge on the unit tests. The programs themselves still have never
  run on ZFS, and M2.6c-3 should not start on them before Q10 has.
- **L3 (F5).** Whether a scheduled agent (M2.7) needs a count of children
  left behind, refusing to start new commands past N. Today it is
  bounded in practice, as F5 explains.
- **L4 (F7).** Drop "no vdev was removed" from the premise's docs, or
  enforce it by refusing a pool with any `remove:` line.
- **L5 (F8).** Whether the client should check who answers
  (`SO_PEERCRED`).
- **L6.** Whether a received property value (source `$recvd`) belongs in
  the manifest. Today it does not.
- **L7.** The time limits, 60 s for a read and 120 s for a change: the
  author's choice, not the design's. They look reasonable: a change
  waits for at least one transaction group, and a suspended pool fails
  fast.

**Can it merge?** Not at `366b217`: B1 and B2 block. With those two fixed
and the doc corrections (F4, F5, F9), yes, given CI green on the new head.
The rest can follow.
