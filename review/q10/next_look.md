# Next look at #148 (M2.6 Q10): the recorder of the ZFS commands the agent's adapter runs

## Heads judged

- **Asked at `0b394b2`; the head moved to `dd75e16` during the look** (one commit, "each record's wall time; the programs on many file systems").
  - `0b394b2..dd75e16` touches only `rec`'s timing (`secs`), the new `scale` function and input, the cleanup's pool list, the record job's timeout (40 → 120 min) and six unit tests.
  - Everything below was re-checked at `dd75e16`, and **line numbers are `dd75e16`'s** unless a line says otherwise. The background record, the `lua` job, the pins and the readings of the recordings were first judged at `0b394b2`; those parts are byte-identical at `dd75e16`.
- Base: `zfs-topology` at `a4e004e`, unchanged.
- **The adapter moved.** #147 is at `0f709ae`, not `366b217`. What changed that matters here:
  - `snapshot.lua`: no `pcall` (B1); a value that is not printable ASCII is returned as hex (B2).
  - `cli.rs`: a **new command**, `zfs destroy -r POOL@T`, run when the snapshot program exits 0 with output that cannot be read (`destroy_taken`, `cli.rs:276-296`, from `snapshot`, `:470-486`).
  - `cli.rs:438-449`: on ZFS 2.2's "missing pool name.", `zpool get` falls back to `zpool status -P POOL`.
  - `cli.rs:496-527`: "tag already exists" is now checked member by member (F3).
  - `host.rs`: a process group, and a cap on children left running. `LC_ALL=C` is unchanged (`host.rs:128`).
  - `zfs.rs`: `HOLD_TAG` is unchanged (`skyzfs-publish`, `zfs.rs:39`).
  - New `cli/recorded/*.json` and `recordings.rs`: #147's fixtures, made from run 45652653655 at `bd37422`. That run predates every fix here (see "For the lead").
  - Every other command's words are unchanged.

**Recordings judged** (read as data, Python with `-I`):

| Run | Head | Images | Lua | Notes |
|---|---|---|---|---|
| 45660525287 | `51fe64e` | 24.04, 26.04 | 366b217 | no delay yet |
| 45661959490 | `b2166b9` | 24.04 (AWS kernel), 26.04 | 366b217 | delay refused (no `zinject`) |
| 45663170240 | `3281028` | 26.04 | 366b217 | delay took |
| 45665823917 | `0b394b2` | 24.04, 26.04 | **0f709ae** | delay took; arrived during the look |

- In the first three runs the `.lua` copies are byte-identical to `366b217`'s.
- **A fourth run came in during the look.** Dispatch 45665823917 ran at `0b394b2`, with `lua_ref` `skyzfs-coding-m2.6c-2`.
  - The `lua` job resolved that to `0f709ae`, and `about.txt` says `lua from 0f709ae…`. The `.lua` copies in both artifacts are byte-identical to `0f709ae`'s.
  - Both image jobs are green (26.04 17:50-17:59Z, 24.04 17:54-18:06Z), and both are judged below. **It is the first run of #147's current `snapshot.lua` on ZFS.**
- Dispatch 45666280714 at `dd75e16` was queued behind it and started at 18:06Z. No recording at `dd75e16` existed when this was written.

## Verdict

**No finding blocks the merge. It can merge once three things hold:**

1. CI is green on `dd75e16`. Everything but the daemon and replay rigs is green; those were still running at 18:07Z.
2. One recording dispatch at `dd75e16` is green. `dd75e16` changes `rec`, which every record goes through, and no dispatch had run at it when this was written; 45666280714 started at 18:06Z.
3. The description is brought up to `dd75e16`: the commit range, 63 unit tests, `secs`, `scale`, the 120-minute timeout, and #147's head.

**D1 is fixed and verified.**
- Every record's own `argv` was fed to #147's adapter at `0f709ae`, on all seven recordings (five with `366b217`'s Lua, two with `0f709ae`'s). Every command the adapter asked for was there under exactly those words: no "no output given".
- `held()` called with `HOLD_TAG` reads the recorded `zfs holds` as held: `Ok(true)` on both pools, on every image and run.
- D2 to D6, G1, G2, G6 and S1 to S8 are fixed as the author says, with the exceptions below. G3 is mostly fixed. G4 is in the description. G5 is recorded, and #147 now reads it correctly.

**New findings, none blocking:**
- **N1:** the adapter's new `zfs destroy -r` is not recorded, and the default `lua_ref` is still `366b217`. Fix both before #147 merges.
- **N2:** the new `scale` input has no bounds. Its `zfs create` loop has no time limit. With the default Lua it measures nothing.
- **N3:** the bare `wait` waits for children that `lim` gave up on.
- **N4:** two statements in the header and the description are contradicted by runs 45663170240 and 45665823917.
- **N5:** a `catch` record has no `secs`.
- **N6:** `index.tsv` is in the order the records ended, not the order they ran.
- **Test gaps T1 to T3:**
  - 15 of 47 plants survive the unit test, among them U7, which the author's answer lists as caught;
  - the locale pin is a grep of one line;
  - the pins are copied by hand from the adapter and have already drifted (N1).

## Findings, most severe first

### Defects

**N1. The adapter moved, and the recorder did not follow.** Certainty: high.

- **What is missing.** At `0f709ae` the adapter runs `zfs destroy -r POOL@T` (`cli.rs:276-296`). It reads three answers:
  - exit 0;
  - "could not find any snapshots to destroy";
  - "dataset does not exist".

  #147 itself says "not recorded on ZFS" (`cli.rs:271-275`).
- **Header and description.** The recorder does not run `destroy`. Its header (`zfs-cli-record.sh:127-130`) and the description ("On `none`: every command the adapter runs") again claim more than is recorded. That is D2's case with a new command.
- **`lua_ref` default.** The default is still `366b217` (`skyzfs-zfs-cli-record.yml:14`, `:62`), and the description calls it "#147's head". It is not.
  - With the default, every `snapshot-*` record shows the `pcall` failure, and `scale` (N2) measures nothing.
  - Every recording but the newest ran `366b217`'s Lua. The newest (45665823917, both images, with `lua_ref` set by hand) is the first in which #147's program ran on ZFS, and it took the snapshot (section 4, "For the lead", item 2).
  - #147's own docs (`cli.rs:105-107`: "the program as it is now has not run on ZFS") are therefore out of date.
- **Fix:**
  - Record `destroy -r`: on each live pool, on a `@t` the program or the CLI took (after the holds are released), on `@absent`, and on `none`.
  - Make the default `lua_ref` #147's branch, or require the input.
  - Dispatch with `lua_ref` at `0f709ae` or later. Dispatch 45665823917 did that at `0b394b2`, green on both images. It does not record `destroy -r` either.

**N2. `scale` (new at `dd75e16`): checked for characters but not bounded. Its file systems are made with no time limit. With the default Lua it measures nothing.** Certainty: high on the first three points (probe `scale_probe.sh`); medium on what a job timeout keeps.

- **Characters are checked, size is not.** The workflow (`yml:104-106`) lets only digits and spaces through, before `sudo`. The script (`:557-558`) skips a word that is not digits, or starts with `0`. No injection is possible:
  - the value reaches the script only through `env:`;
  - it is used only as `seq`'s bound and in file and record names;
  - it is never used in arithmetic, in `eval` or in a glob.

  But nothing bounds a count or the number of counts. `99999999999999999999` passes both checks, and so do 20 counts (`scale_probe-dd75e16.out`).
- **The time it can take.**
  - `seq 1 "$n" | xargs -P 8 -I{} zfs create …` (`:371`) runs with no `lim`.
  - Then five records run with a limit of 1800 s each (`:361`, `:374-378`), and `zpool destroy` with 600 s (`:379`).
  - One count can therefore exceed the job's 120 minutes (`yml:75`) by its records alone (5 × 1800 s = 150 min), before its `zfs create` loop, which has no bound.
  - When the timeout cuts a record, that record is left without `rc`, `secs` or a line in the index: `rec` writes those only once `lim` returns (`:176-188`).
- **What a timeout keeps.**
  - The ordinary records are all written before `scale` starts (`:556`), on other pools and under other names, so a scale run cannot change them. It only adds lines to `about.txt` and `create-skzqs-N.log`.
  - They reach the artifact after a timeout only if the `if: always()` steps run on the cancellation. GitHub does run those on cancellation, as I understand it; this was not tried.
- **Disk.** `scale.img` is a sparse 16 GiB file (`:362`), so the host needs up to 16 GiB free beside the roughly 9 GiB the other images take. Nothing checks it.
  - If the host's file system fills first, writes to the file vdev fail, and the pool most likely suspends (failmode `wait`).
  - The `zfs create` loop, which has no limit, would then hang until the job's timeout.
  - If ZFS's own 16 GiB fills first, `zfs create` fails with "out of space", which is harmless.
- **The default Lua.**
  - With the default `lua_ref` (`366b217`), `scale-N-snapshot` fails at `pcall` at once and takes no `@t`. `members`, `hold -r`, `tagged` and `release -r` are then measured on a snapshot that does not exist.
  - The same happens with `0f709ae`'s Lua wherever the program reaches its instruction limit before the take, which is what a large N is there to find.
  - `record_pool` falls back to `zfs snapshot -r` (`:306`); `scale` has no fallback.
- **Smaller points.**
  - The same count given twice builds and destroys a second pool for nothing, since its records are refused as "twice".
  - The unit test pins that `LIMIT` comes back afterwards (V18 caught). It does not pin that `scale`'s records ran under `ZCR_SCALE_LIMIT`: plant V19, which drops the `local LIMIT`, survives.
- **Suggested fixes:**
  - bound the count (for example ≤ 100 000) and the number of counts (for example ≤ 3) in both checks;
  - run the `zfs create` loop under `lim` with a budget, and stop starting records once a total budget (for example 90 minutes) has passed;
  - take `@t` with `zfs snapshot -r` when the program did not, as a timed record of its own;
  - write `df` of `$WORK` into `about.txt` before the scale pool is made.

**N3. A bare `wait` waits for children `lim` gave up on.** Certainty: high on the mechanism (probe W1); low on the chance of it here.

- Where: `:528` and `:534`.
- `lim` gives up on a child it cannot kill (one in the kernel's wait) and leaves it as a job of the calling shell (`tests/lib/lim.sh:12-13`, `:31`).
- `catch` runs `lim` in `main`'s own shell. A `zpool status` that `catch` abandoned is therefore waited for by the bare `wait`, and that `wait` has no limit.
- Probe W1 makes `kill` a no-op. `catch` returned after 1 s; the bare `wait` returned 5 s later, when the abandoned child ended, although the background record had ended at 2 s. With `wait "$!"` (W2) it returned 1 s after `catch`, when the record ended (`wait_probe-dd75e16.out`).
- Fix: save `$!` after each `rec … &` and `wait` for that pid. Nothing else in the background is meant to be waited for.

**N4. Two statements that runs 45663170240 and 45665823917 contradict.** Certainty: high on the first, medium on the second.

- **"On files, `zpool online` and `zpool attach` return only once the resilver has ended."** This is in the header (`:121-123`), the description and the author's answer (G3).
  - With the delay, `mirror-online` returned at 17:21:39.86, about 4 s after the resilver began (17:21:36).
  - That resilver ended at 17:23:01 (`mirror-resilvered`: "resilvered 1G in 00:01:25"), 81 s after `online` had returned.
  - Run 45665823917 (26.04, at `0b394b2`) shows the same thing. `online` returned at 17:54:40.03, and the resilver, which began at 17:54:37, ended at 17:56:00 ("resilvered 1G in 00:01:23").
  - In the runs without the delay, `online` returned within a second of the resilver's end, and `attach -s` with the end of the scrub that follows a rebuild. That only shows that short scans end inside the command's own wait.
  - Running the commands in the background is harmless, but the stated reason is not what happens.
- **The rebuild "most likely not caught" because `zpool status` "waited behind the attach until the rebuild had ended"** (`:113-119`).
  - The status did wait behind the attach: try 2 returned at 17:23:11.80, the attach at 17:23:11.84.
  - But the rebuild was not slowed at all. It copied 2 GiB in 6 s, against 3 to 11 s in the runs without the delay, while the resilver was slowed from about 3-9 s to 85 s.
  - Most likely the delay is only on `m0`, and a rebuild from a two-way mirror reads mostly from the undelayed `m1`. The resilver of `m1` had to read `m0`.
  - So the rebuild ended before the attach returned. With the delay on `m1` as well (or on both), the rebuild would most likely outlast the attach, as the resilver outlasted `online`, and `resilver (mirror-0) in progress` could be caught.
  - Run 45665823917 again: a rebuild of "2G in 00:00:10", ending at 17:56:10, against a resilver of 83 s and a scrub after it of 2:46. The attach returned at 17:56:15, and its line in the index comes before the catch's records. With `0b394b2`'s narrower pattern, the 100 tries saw only the scrub, and both notes now say "not caught", correctly. Its 24.04 job is alike: `online` returned at 18:01:45 and the resilver ended at 18:03:10; a rebuild of "2G in 00:00:12" ended at 18:03:23, and the attach returned at 18:03:27.
  - The adapter's reading of a rebuild under way is still on #147's list of readings not checked on ZFS (`cli.rs:124-126`).

**N5. A `catch` record has no `secs`.** Certainty: high.

- `keep` (`:226-243`) writes no `secs`, so no scan-state record (`*-scrub-status`, `*-resilver-status`, `remove-in-progress`, …) has one. Yet the header (`:22`) lists `secs` for every record.
- `rec`'s `secs` brackets the `lim` call alone (`:175-179`). It is the command plus `lim`'s two `mktemp`, its fork and the copying of the outputs, in steps of `lim`'s 0.1 s poll.
- So `secs` measures the adapter-visible command, not setup, to about 0.1 s. That is enough for `scale`'s minutes; it is not enough to compare sub-second commands.
- `scale`'s file systems are timed apart, in whole seconds, into `about.txt` (`:370-373`).

**N6. `index.tsv` is in the order the records ended.** Certainty: high (probe W4; run 45663170240's index has `mirror-online` after `mirror-resilver-status-g`, and `mirror-rebuild` between the rebuild status and its `-g`).

- The header (`:27-28`) says "in the order run".
- The lines themselves are whole: one `printf` each, appended.
- Say "in the order each ended", or write the background record's line when it starts.

### Test gaps

**T1. The pins cover one call site of each command, and the locale pin is a grep.** Certainty: high.

- `review/q10/plants_next.sh dd75e16`, output in `plants-next-dd75e16.out`: 47 plants, each applied alone to a copy of the head's script and the head's unit test run on it.
- **32 caught, 15 survive.**
- **Survivors that change the adapter's words:**

  | Plant | Change |
  |---|---|
  | V2 | `export LC_ALL=C.UTF-8` added after the pinned line |
  | V3 | `rec` runs its command with `LC_ALL=C.UTF-8` |
  | V4 | `-n` dropped on `tagged-held` |
  | V5 | `-n` dropped on `none-members` |
  | V6 | `-r` dropped on `hold-2` |
  | V7 | `-r` dropped on `release-2` |
  | V8 | the old tag on `none-hold` |
  | V9 | `rig-resilvered` without its `-g` partner |
  | V17 | `-n` dropped on `scale-N-members` |

- **Other survivors:**

  | Plant | Change |
  |---|---|
  | U7 | `catch` keeps an earlier try's rc |
  | V19 | `scale` ignores its own limit |
  | V22 | the bare `wait` dropped |
  | V26 | `keep` writes `argv` with spaces, not NUL |
  | V29 | `--expect` dropped from `hold-2` |
  | V30 | the rebuild's pattern reverted to `in progress` |

- **Caught:**
  - the tag (U12);
  - `-P` (U9), `-p` (U10), `-H` on `holds -r` (U11) and on `holds` (V15);
  - `-g` (V12);
  - `-n` on `members` (U13), and `-n` added to the snapshot program (V11);
  - the program's limits (U2, V10) and `-j` (U1);
  - the assist (V13, V14);
  - `export LC_ALL=C.UTF-8` in place of `C` (V1);
  - `scale`'s `hold -r` (V16);
  - `secs` (V20, V21);
  - `scale`'s `LIMIT` leaking out (V18) and its image left behind (V23);
  - `catch`'s limit (U5), its stop on 124 (V24) and its scratch (V25);
  - `keep`'s refusal of a name used twice (U8);
  - `--expect` (V27, V28).
- **U7 is not caught.** The author's answer lists it as caught.
  - `catch` resets `rc` on every try (`:204`). No test has a try that fails followed by one that succeeds, so `[ "$i" = 1 ] && rc=0` passes all 63 tests.
  - Effect if it happened: a status caught at try 2, after a try 1 that failed, would be kept with try 1's rc.
- **Why the rest survive:**
  - Each command's words are pinned at one call site (`rig-…`).
  - `main`'s own records never run in the test: the absent pool, the changes of devices and the rebuild pattern.
  - The locale is pinned by `grep -c '^    export LC_ALL=C$'` (test `:171`), which says nothing of the environment the commands get.
- **Suggestions:**
  - Loop over every record the test makes and check each one's `argv` by rule rather than by name:
    - each `members.lua` and `tagged.lua` program has `-n`, and the snapshot program has none;
    - each `hold` and `release` is `-r "$TAG"`;
    - each `zpool status` has `-P` and comes as a pair.
  - Move the absent pool's records into a function (`record_none`) so the test can run them.
  - Have the fake `zfs` and `zpool` print `$LC_ALL` into their stdout, so the locale is pinned by what the commands saw.

**T2. The pins are copied by hand from the adapter, the test does not say what to do when the adapter changes, and they have already drifted.** Certainty: high.

- The test writes the words out (`test_zfs_cli_record.sh:122-125`: "written out here … A word the recorder changes fails here"). Nothing checks them against #147's source, which is not in this tree, and nothing says to re-check them when #147 changes.
- #147 has since added `zfs destroy -r` (N1), and nothing flagged it.
- The other side exists in #147: `recordings.rs` checks that the adapter's words are each record's `cmd`. But:
  - it reads #147's own `recorded/*.json`, made from run 45652653655 at `bd37422`;
  - it passes the recorder's old tag (`const TAG: &str = "skyzfs"`, `recordings.rs:41`) as the tag argument, so `HOLD_TAG` is not pinned there either.
- Suggest one line in the test's comment: when #147's `cli.rs`, `zfs.rs` or `host.rs` change a command, change it here and in the script, dispatch, and refresh #147's `recorded/*.json` from that run.

**T3. Still not recorded.** Certainty: high.

- A rebuild in progress (see N4 for how it might be caught).
- A checkpoint of `0`.
- (Now recorded on both images, in run 45665823917: the snapshot program's success shape, `createtxg`, and the values beyond ASCII. See section 4, "For the lead", item 2.)
- On both images, still missing:
  - a program that reaches its instruction limit (N2);
  - output from the program that cannot be read, which is the case that runs `destroy -r` (N1).
- **Warning for fixture authors.** Run 45663170240's `mirror-rebuild-status` and `-g` carry the note "caught at try 2/1", but they show the scrub that follows the rebuild ("scan: scrub in progress" over "scan: resilvered (mirror-0) …"). Taken from that run, they are not a rebuild under way. From `0b394b2` on, the pattern no longer accepts them: run 45665823917 keeps the same kind of scrub under "not caught: no output of 100 matched …".

### Suggestions

- **S9.** `wait "$pid"` for the background record (N3).
- **S10.** Delay `m1` as well during the rebuild, to try for its state in progress (N4).
- **S11.** Bound `scale` and give it a fallback snapshot and a time budget (N2).
- **S12.** `secs` in `keep` too, or narrow the header (N5).
- **S13.** The pins by rule, the absent pool in a function, the locale through the fakes (T1), and the comment on adapter changes (T2).
- **S14.** The description: the commit range (bd37422..dd75e16), 63 unit tests, `secs` and `scale`, the 120-minute timeout, #147's head (`0f709ae`), and the line about `online` and `attach` (N4).

## 1. The pins against the adapter's words (judged at `0b394b2` and `dd75e16`)

The adapter at `0f709ae`, against the recorder at `dd75e16` and its unit test:

| | Adapter (`0f709ae`) | Recorder | Unit test pin |
|---|---|---|---|
| `zpool status` | `-P POOL`, `-P -g POOL` | `status()` `:265-268`, always the pair | `rig-status`, `rig-status-g`, scrub pairs |
| `zpool get` | `-H -p -o property,value guid,load_guid,multihost,readonly,checkpoint POOL` | same (`:275`) | `rig-get` |
| `zpool get` on 2.2, absent pool | then `zpool status -P POOL` | `none-status` | (`agent_commands`) |
| snapshot program | `zfs program -j -t 100000000 -m 104857600 POOL - POOL T skyzfs:assist` | same | `rig-snapshot-1`, `scale-3-snapshot` |
| members, tagged | `zfs program -n -j …` | same | `rig-members`, `rig-tagged` (not `tagged-held`, `none-*`, `scale-*-members`) |
| `zfs holds` | `-H POOL@T`; `-H -r POOL@T` | same | `rig-holds`, `rig-holds-r` |
| `zfs hold`, `zfs release` | `-r skyzfs-publish POOL@T` | same, `TAG` (`:148`) | `rig-hold-1`, `rig-release-1`, `scale-3-hold`, `scale-3-release` (not `hold-2`, `release-2`, `none-*`) |
| assist | `zfs set skyzfs:assist=V POOL`; `zpool sync POOL` | same | `rig-assist-*`, `none-assist-set` |
| **`zfs destroy -r POOL@T`** | **new** (`cli.rs:276-296`) | **not recorded** | none (N1) |
| locale | `LC_ALL=C` (`host.rs:128`) | `export LC_ALL=C` (`:402`); the `locale` record shows `LC_ALL=C` in all seven recordings | a grep of the line (T1) |
| stdin | the Lua through a pipe | `< file.lua` | `rig-snapshot-1`'s `cmd` |

- The pins are derived from neither side. They are written out by hand, and the test says nothing about the adapter changing (T2).
- Plants of the kinds asked for:
  - **tag changed:** U12 caught;
  - **flag dropped:** U9, U10, U11, V12, V15 caught; V6, V7, V9 survive at uncopied call sites;
  - **locale changed:** V1 caught; V2 and V3 survive;
  - **`-n` dropped:** U13 caught; V4, V5 and V17 survive.

## 2. The background record, `catch` and `wait` (judged at `0b394b2`; unchanged at `dd75e16`)

- **Two records writing the same file.**
  - The background `rec` writes `OUT/mirror-online/` (or `mirror-rebuild/`).
  - `catch` writes `OUT/.catch` and moves it to its own name.
  - They share only `index.tsv` and stdout, each line appended by one `printf`. The index lines of all seven recordings are whole.
  - The `EXIT` trap does not run in the background subshell (bash resets it there; checked), so a background record ending cannot destroy the pools.
- **A record that ends before `catch` starts.**
  - `catch` keeps the state after, with "not caught …", and returns 1 (probe W3).
  - The not-caught path is still flagged when a record runs in the background: `catch` runs in the foreground and is unchanged.
  - The background records have no `--expect`; their own rc is in their record.
- **`wait` and failures.**
  - A bare `wait` returns 0 whatever the background job returned.
  - The script has no `set -e` (`set -uo pipefail`, `:134`), so nothing is lost: the background record's rc is in its files.
- **A job that hangs.**
  - The background `rec` is bounded by its own `lim` (120 s).
  - The bare `wait` is not bounded: it also waits for anything `lim` left behind in `main`'s shell (N3).
- **The interleaving in run 45663170240** (from the job's log times and the records' `scan:` lines):

  | Step | What happened | Intended? |
  |---|---|---|
  | Mirror scrub | started 17:20:00.45. `-P` caught at try 1 at 17:20:00.58, `-g` at 17:20:00.72. It ran to about 17:21:23 (83 s; 1-5 s without the delay) | yes |
  | Mirror resilver | began 17:21:36. `-P` caught at try 2 at 17:21:37.12, `-g` at try 1 at 17:21:37.25. `online` returned at 17:21:39.86; the resilver ended at 17:23:01 | yes |
  | Rebuild | `catch` try 1 ran before the attach, idle. Try 2 waited behind the attach until 17:23:11.80. The rebuild had run 17:23:01-17:23:07, unslowed; the scrub after it was slowed (to 17:25:52). The kept output is that scrub | **no** (N4) |
  | Removal (`skzqr`, no delay) | caught at try 1 in both forms, at 64 KiB of 387 MiB | yes |

  So the delay works for the scrub and the resilver, and does nothing for the rebuild.

## 3. The workflow (judged at `dd75e16`)

- **The commit handoff.** The `lua` job resolves `lua_ref` once (`yml:60-68`) and outputs `git rev-parse FETCH_HEAD`.
  - Each record job refuses anything but hex (`:94-96`). It fetches that commit by id (`:97`) and reads the three files from that `FETCH_HEAD` (`:99-101`).
  - The name is never fetched again, so there is no window between jobs. `about.txt` names the id (`lua from …`), and the record step receives it through `sudo --preserve-env`.
  - The Lua is not passed as an artifact. Fetching an immutable commit id is equivalent.
  - An empty output fails the case (`''`).
  - The hex check does not insist on 40 characters, but `rev-parse` always gives 40.
- **The case patterns.**
  - `lua` job: `-*|*[!A-Za-z0-9._/-]*|''` (`:65`). No option and no refspec syntax can pass; `..` and the like are refused by git.
  - Record job: hex only.
  - `scale`: digits and spaces (N2).
- **Inputs reach scripts only through `env:`.** No `${{ }}` appears in any `run:`. `matrix.image` appears in `name`, `runs-on`, `IMAGE` and the artifact's `with:` only.
- **Permissions.**
  - `contents: read` at the top.
  - `persist-credentials: false` on all three checkouts (`:46`, `:59`, `:87`).
  - No secrets are used.
- **`apt-fallback`.** It runs right after checkout in `record`, the only job that installs packages.
- **Dispatch only.**
  - `lua` and `record` are gated on `workflow_dispatch` (`:51`, `:73`).
  - A push to `skyzfs-testing*` runs `unit` only (10 min).
- **Timeouts.** `lua` 5 min; `record` 40 → 120 min at `dd75e16`, for every dispatch and not only those with `scale`. An ordinary run took 8 to 9 minutes.

## 4. D1-D6, G1-G6, S1-S8 against the code and the recordings

- **The test.** `q10n_scratch.rs` feeds each record to #147's `ZfsCli` at `0f709ae`, under the words of the record's own `argv`, and calls it with the adapter's constants.
- **The outputs** are in `readings-next-<run>-<image>.txt`, one per recording.
- **No reading asked for a word the recorder did not run.** Each output lists the words the adapter ran ("ran:").

| Item | State at `dd75e16` | Evidence |
|---|---|---|
| D1 | fixed | `TAG=skyzfs-publish` (`:148`); hold, release, holds and `tagged.lua` under the adapter's words. `held(…, HOLD_TAG)` on `*-holds` → `Ok(true)`; `tagged` → `{}` then `{"t"}`; all seven recordings |
| D2 | fixed | `assist-set`, `assist-sync` on `rig`, `mirror` and `none` (`:276-277`). `assist()` → `Ok(())` on live pools; on `none`, `Other("… dataset does not exist")` |
| D3 | settled on both images | `snapshot-exists-child` → `Err(Exists)` on 24.04 and 26.04, nothing taken (`exists-child-after`). `snapshot-2` → `Err(Exists)` after the CLI fallback took `@t` |
| D4 | fixed | `rec --expect` (`:154-190`) on five records; "seen" 10 of 10 in every recording, none "NOT SEEN". V27, V28 caught; V29 survives (T1) |
| D5 | fixed | `export LC_ALL=C` (`:402`); the `locale` record shows `LC_ALL=C` and every `LC_*` `"C"` in all seven. Pin weak (T1) |
| D6 | fixed | every status is a pair, including `*-resilvered`, `mirror-rebuilt` and `remove-*`; all present in every index |
| G1 | fixed | `holds-r-released`: empty, rc 0. `release-2` + it → `Ok(())` |
| G2 | fixed | `holds-absent`, `hold-absent`, `release-absent`: "dataset does not exist", rc 1 → `held` false, `hold` `NoSnapshot`, `release` `Ok(())` |
| G3 | mostly | Headers recorded and read: special 1, dedup 1, other 2 (cache + spares). Removal: in progress → busy and removed; done → removed only. Rebuild in progress and checkpoint 0 not recorded; the program's readings are recorded on both images in run 45665823917 (T3, N4) |
| G4 | in the description | 24.04 is the 2.2.2 userland over a 2.4.1 module (`about.txt`) |
| G5 | recorded; #147 now right | `hold-r-partial`: exit 1, "tag already exists", root only held. #147 → `Err(Other("… not on skzq/child@t, skzq/vol@t"))`. `release-r-partial`: exit 0, child released → `Ok(())` |
| G6 | partly | pins exist; 32 of 47 plants caught (T1) |
| S1, S2, S8 | done | section 3 |
| S3 | resolution done; default not | the default `lua_ref` is still `366b217` (N1) |
| S4 | done | V24 caught |
| S5 | done | header `:18-20` |
| S6 | done | V25 caught |
| S7 | done for `rec` | `keep`'s `argv` unpinned (V26) |

### For the lead: the recordings against #147's readings

1. **`zpool get` on a pool that does not exist.**
   - 24.04's 2.2.2 userland exits 2 with "missing pool name." and its usage.
   - 26.04 exits 1 with "Cannot get properties of skznone: no such pool available."
   - #147 at `0f709ae` asks `zpool status -P` after the first. It reads `NoPool` on all seven recordings.
2. **`pcall`.** The sandbox has none: "attempt to call global 'pcall' (a nil value)" at line 42, on both images.
   - #147 removed it at `0f709ae`.
   - **#147's current `snapshot.lua` has now run on ZFS, on both images** (run 45665823917; `readings-next-45665823917-ubuntu-{24.04,26.04}.txt`). 24.04's 2.2.2 userland gives the same shapes as 26.04. What it gave:
     - **The live pools.** Exit 0 and `{"return":{"createtxg":"32","members":{NAME:{"guid":…,"snapshot_guid":…,"type":"filesystem"|"volume","local":{…}}}}}`.
       - `createtxg` and every GUID are decimal strings.
       - A GUID above 2^63 comes as a negative number (`"guid":"-992711124556426838"`).
       - #147 reads `Ok(Program { createtxg: 32, … })`, takes the negative GUID as the u64 it stands for (17454032949153124778), and gets the same `snapshot_guid` for each member as `members.lua` gives.
     - **`snapshot-odd`.** Values that are not printable ASCII come as `{"encoding":"hex","value":…}`, for example `skyzfs:raw` as `61fffe62c3`. #147 reads exactly those bytes, for `skyzfs:raw` and `skyzfs:utf8` alike.
       - Under `LC_ALL=C.UTF-8` (`snapshot-odd-utf8`) the output is the same apart from the new snapshot's GUIDs and `createtxg`.
     - **An existing snapshot**, on the root (`snapshot-2`) or on a child only (`snapshot-exists-child`): exit 0 and `{"return":{"failed":"skzq@t","errno":"17"}}` (`"failed":"skzq/child@p"` for the child) → `Err(Exists)`.
       - `exists-child-after` shows nothing taken: only the child's own `@p`.
     - **The pool that does not exist:** exit 1, "cannot open 'skznone': no such pool" → `NoPool`.
     - Exit 0 with output that cannot be read did not happen, so `destroy -r` was not exercised (N1).
3. **A partial hold.** With the tag on the root's `@t` only, `zfs hold -r` exits 1 with "tag already exists", naming the root, and adds the tag to no child.
   - #147 at `366b217` read that as held. At `0f709ae` it reads the members and their holds, and refuses (`Other`), on every recording.
   - A partial release (child only) exits 0 and releases the child.
4. **A pool that does not exist:**
   - `zfs hold`, `holds` and `release` say "dataset does not exist", read as `NoSnapshot`, `false` and `Ok(())`;
   - `zfs set` (the assist) says the same, read as `Other`;
   - `zpool status`, `zpool sync` and the programs say "no such pool", read as `NoPool`.

   #147 documents this; the engine ignores the assist's result.
5. **#147's fixtures are not these recordings.** `recorded/*.json` come from run 45652653655 at `bd37422`: the old tag `skyzfs`, `-g` borrowed between states (`GUIDED`), and records made in `C.UTF-8`. They should be refreshed from a run at `0b394b2` or later with `0f709ae`'s Lua, with `TAG = HOLD_TAG`, and `GUIDED` dropped. Run 45665823917 would do; a run at `dd75e16` adds `secs`.
6. **The new `zfs destroy -r`** is not recorded (N1).

## 5. Gates and CI

**Local:**
- `bash tests/kernel/test_zfs_cli_record.sh`: 57 of 57 at `0b394b2`, 63 of 63 at `dd75e16`, exit 0.
- **shellcheck.** The local one is 0.9.0, older than CI's and not a gate. ShellCheck 0.11.0 was run as CI runs it, `git ls-files '*.sh' | xargs -r shellcheck`: clean at `0b394b2` and at `dd75e16`.
- **Python tests.** No Python changes in this PR, so none were run.
- **Rust.** No Rust changes in this PR. `q10n_scratch.rs` built and ran on a scratch checkout of `0f709ae` (toolchain 1.100.0): `cargo test -p skyzfs-agent --lib q10n_scratch`.

**CI on `dd75e16`, at the time of writing:**

| Run | State |
|---|---|
| `skyzfs-ci` push 45666269570 | green |
| `skyzfs-zfs-cli-record` push 45666269591 (unit) | green |
| `ci` 45666281933 (lint with shellcheck and the unit test, test, unit-arm64, …) | green |
| `e2e` 45666281758 | green |
| `skyzfs-kernel-probe` 45666282115 | green |
| `coverage` 45666281818 | green |
| `skyzfs-daemon-rig` 45666281924, `skyzfs-replay-rig` 45666281748 | running (18:07Z) |
| recording dispatch 45666280714 | started 18:06Z |

- On `0b394b2`, `ci`, `coverage`, `e2e` and the kernel probe were green; the rigs were cancelled by the newer push.
- **Codex** posted a failed-review notice and a usage-limit notice on `bd37422`, and nothing since.

## Files beside this note

- `q10n_scratch.rs`: #147 at `0f709ae` fed the records under their own `argv`. Its header says how to run it.
- `readings-next-<run>-<image>.txt`: its output on the seven recordings.
- `plants_next.sh`: the plants (`bash review/q10/plants_next.sh HEAD WORKDIR`). Output: `plants-next-dd75e16.out`.
- `wait_probe.sh`: the background, `catch` and `wait` probes (`bash review/q10/wait_probe.sh HEAD WORKDIR`). Output: `wait_probe-dd75e16.out`.
- `scale_probe.sh`: the `scale` checks, taken from the head's files (`bash review/q10/scale_probe.sh HEAD`). Output: `scale_probe-dd75e16.out`.
- The previous review and its files: `review.md`, `q10_scratch.rs`, `plants.sh`, `catch_probe.sh` and their outputs.
