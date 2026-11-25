# Review of #148 (M2.6 Q10): the recorder of the ZFS commands the agent's adapter runs

## Heads judged

- **The head moved during the review.** I began on `bd37422`. The pull request's head became `73dce38` (one more commit, "Recorder: values beyond ASCII and not UTF-8, the locale, a partial take").
- I reviewed that commit's delta and re-checked every finding below at `73dce38`. Line numbers are `73dce38`'s.
- Base: `zfs-topology` at `a4e004e`.
- Diff: `git diff origin/zfs-topology...73dce38`. It holds:
  - `tests/kernel/zfs-cli-record.sh` (new);
  - `tests/kernel/test_zfs_cli_record.sh` (new);
  - `.github/workflows/skyzfs-zfs-cli-record.yml` (new, the same in both commits);
  - one line each in `ci.yml` and `skyzfs-ci.yml`.

**Adapter compared against.** #147 at `366b217`, which is still its head: `crates/skyzfs-agent/src/zfs/cli.rs`, `zfs.rs`, `host.rs` and the three `.lua` files.

**Recordings judged** (read as data):

| Run | Head | Images | Artifacts |
|---|---|---|---|
| dispatch 45652653655 | `bd37422` | 24.04 and 26.04 | `zfs-cli-ubuntu-24.04`, `zfs-cli-ubuntu-26.04` |
| dispatch 45656199148 | `73dce38` | 26.04 only | `zfs-cli-ubuntu-26.04` |

Both used `lua_ref` 366b217, and the `.lua` copies in the tarballs are byte-identical to 366b217's.

## Verdict

**Not ready to merge. D1 blocks.**

- **D1:** the recorder runs the hold commands and `tagged.lua` with the tag `skyzfs`. The adapter runs them with `skyzfs-publish`.
  - So 7 kinds of record a pool, and 3 on the absent pool, are not the command the adapter runs.
  - The `zfs holds` records carry the wrong tag. Fed to the adapter's `held()` with its own tag, they read "not held".
  - The fix is one word and a fresh dispatch. A fresh dispatch is due anyway for #147's next head, since `snapshot.lua` fails at 366b217.
- **D2, D4 and D5** should be fixed in the same round. None of them blocks alone.
  - D5 is also a wrong statement in the new commit: "the adapter inherits its locale". At 366b217 it sets `LC_ALL=C` on every command.
- **D3 is settled at `73dce38`** for 26.04. `snapshot-partial` records the EEXIST return, and the adapter reads it as `Exists`.
- The recorder itself is sound:
  - `rec` and `catch` keep stdout, stderr and the exit code whole;
  - the not-caught path is recorded and flagged;
  - the workflow's `lua_ref` handling is safe;
  - shellcheck and the unit test are green in CI on both heads.
- The first recording already found two things in #147:
  - `pcall` is missing from the channel-program sandbox;
  - on the 24.04 userland, `zpool get` on a pool that does not exist reads `Other`, not `NoPool`.

## Findings, most severe first

### Defects

**D1 (blocks). The hold tag is not the adapter's.** Certainty: high.

- Where:
  - `tests/kernel/zfs-cli-record.sh:284-291` (`tagged`, `hold-1`, `holds`, `holds-r`, `hold-2`, `tagged-held`, `release-1`, `release-2`);
  - `:337`, `:340`, `:341` (`none-tagged`, `none-hold`, `none-release`).
- The adapter's tag is `HOLD_TAG = "skyzfs-publish"` (`crates/skyzfs-agent/src/zfs.rs:38` at 366b217). It is used for every hold, release, `held` and `tagged` call (`engine.rs:501, 506, 550, 637, 721, 927`).
- The recorder runs `zfs hold -r skyzfs skzq@t` where the adapter runs `zfs hold -r skyzfs-publish skzq@t`. The same goes for `release` and for `tagged.lua`'s second argument.
- What it costs:
  - The `holds` and `holds-r` stdout records name the wrong tag (`skzq@t<TAB>skyzfs<TAB>…`).
  - The adapter's fake host is keyed by the command's words, so the `cmd` files do not match what `ZfsCli` asks for.
  - Fed to `ZfsCli::held(pool, "t", HOLD_TAG)`, the record gives `Ok(false)` on every image and pool recorded. With the recorder's tag it gives `Ok(true)` (`review/q10/readings-*.txt`, rows `held … tag skyzfs-publish`).
  - So the records cannot replace the hand-written fixtures without being edited. Editing them defeats the work package's point ("what ZFS writes, not what we expect it to write").
- The messages themselves ("tag already exists", "no such tag") do not name the tag, so the readings drawn from them stand.
- The unit tests do not catch a change of tag: plant U12 survives (G6).
- Fix: use `skyzfs-publish` as the adapter does, and re-dispatch.

**D2. The assist's commands are not recorded.** Certainty: high that they are missing; medium on what ZFS would print.

- Where:
  - `zfs-cli-record.sh:176-182` and `:276-315` (the commands recorded);
  - `:72-75` (the header).
- `Zfs::assist` (`cli.rs:393-397`) runs `zfs set skyzfs:assist=V POOL`, then `zpool sync POOL`.
- Neither is recorded. They are not recorded on a live pool, and not on the absent pool.
- The new `zfs set` records (`set-utf8`, `set-raw`) set other properties on the child. They are not the assist.
- The header says that on the absent pool the recorder runs "every command of the agent's", and the PR description says "each of the adapter's commands as the adapter runs them". Both claims are wider than what is recorded.
- What a recording would pin:
  - On an absent pool, `zfs set` presumably fails at `zfs_open` with "dataset does not exist". `assist` runs it with `snapshot = false`, so that reads as `Other`, not `NoPool`.
  - On a live idle pool: the exit codes of the set and the sync.
- The engine ignores `assist`'s result (`engine.rs:707-715`), so today's cost is low. #147's own request to testing did not list the assist, so this is scope. Either record it (two `rec` lines a pool, two on `none`) or narrow the header and the description.

**D3 (settled at `73dce38` for 26.04). The EEXIST reading was not exercised at `bd37422`.** Certainty: high.

- At `bd37422`, `snapshot-2` ran before the `snapshot-cli` fallback took `@t` (`:279`, `:281`). Both `snapshot-2` records were a second `pcall` failure, so EEXIST was exercised on neither image.
- `73dce38` adds `partial-pre`, `snapshot-partial` and `partial-after` (`:300-302`). It takes `child@p`, then runs the program at `@p`.
- Run 45656199148 recorded:
  - `{"return":{"failed":"skzq/child@p","errno":"17"}}`, exit 0;
  - `partial-after` shows no `@p` but the child's.
- The adapter reads that as `Err(Exists)` on both pools (`readings-ubuntu-26.04-73dce38.txt`).
- Two small things remain:
  - **The name.** The record and its comment (`:49-53`, "a take that fails part way") are not a partial take with this program. `snapshot.lua` checks every member before it takes any, so this is the EEXIST check failing before anything is taken. The `taken` path (a failure after some were taken) is still not reachable, and not recorded.
  - **24.04.** It has not been recorded at `73dce38`.

**D4. `rec` keeps no expectation, so a record that missed its state is not flagged.** Certainty: high (plant P1).

- Where: `zfs-cli-record.sh:94-115`, against `catch`'s notes at `:133-134`.
- Several records exist to show one message:
  - `snapshot-2` (EEXIST);
  - `hold-2` ("tag already exists");
  - `release-2` ("no such tag");
  - `members-absent`;
  - the `none-*` records;
  - now `snapshot-partial`.
- When the command does something else, `rec` records it under the same name with no note, and the summary shows only the rc.
- Real instance: every `snapshot-2` recorded so far is a second `pcall` failure. The header still documents it as "the second: EEXIST" (`:33`), and nothing in the index or the summary says otherwise.
- Planted (`review/q10/catch_probe.sh`, P1): a "second hold" that succeeds is kept as `rc=0`, `note=NONE`, summary row `| x-hold-2 | 0 |  |`.
- It is recorded as such and nothing falls over, but it is not flagged.
- Suggest: an optional expected pattern on `rec` (like `catch`'s) that writes a note "expected /…/, not seen".

**D5. The locale is not the adapter's, and the new comment says the adapter inherits its locale.** Certainty: high.

- Where:
  - `zfs-cli-record.sh:111` (`rec`) and `:129` (`catch`): no `LC_ALL`;
  - `:196` (`main` exports only `DEBIAN_FRONTEND`);
  - `:44-48` (the new comment);
  - `skyzfs-zfs-cli-record.yml:73` (`sudo --preserve-env=LUA_REF`, which keeps sudo's default locale variables).
- The adapter runs every command with `LC_ALL=C` (`host.rs:88` at 366b217; its module docs say "so that its messages can be read"; #147's plant H4 pins it).
- The recorder runs every record but three in the job's locale. The new `locale` record shows that locale is `LANG=C.UTF-8`, with every `LC_*` `C.UTF-8` (run 45656199148).
- `73dce38` adds `program --locale L` and records `snapshot.lua` three ways:
  - in the job's locale;
  - with `LC_ALL=C`;
  - with `LC_ALL=C.UTF-8`.

  Its reason, "the adapter inherits its locale", is wrong at 366b217. Of the three, only `snapshot-odd-c` is the adapter's command. If #147 means to move to `C.UTF-8`, say so in both places.
- Why it matters:
  - The values beyond ASCII that `73dce38` sets are what `zfs program -j` may write differently in `C` than in `C.UTF-8`.
  - The holds date and the decimal mark of `zpool status` also follow the locale.
  - For the records so far it most likely changes nothing: `C.UTF-8` formats these as `C` does, and OpenZFS ships no message catalogues.
- All three `snapshot-odd*` records fail at `pcall` at 366b217, so they show nothing yet.
- Fix: `export LC_ALL=C` in `main`, and keep `--locale` for deliberate variants.

**D6 (minor). Some states have no `-g` partner.** Certainty: high.

- Where: `zfs-cli-record.sh:312`, `:314`, `:324`, `:332` (`scrub-paused`, `scrub-canceled`, `rig-resilvered`, `mirror-resilvered`).
- `Zfs::layout` always reads `-P` and then `-P -g`, but these states are recorded with `-P` only.
- A fixture has to borrow another state's `-g`:
  - for the scrub states the idle one, whose shape is the same;
  - for `rig-resilvered` the resilver's, since the shape changed from one disk to a mirror.
- Add `-g` for each.

### Test gaps

**G1. `zfs holds -H -r` after the release is not recorded.** Certainty: high.

- After "no such tag", `release` runs `zfs holds -H -r pool@t` and needs it to show no member with the tag (`cli.rs:339-364`).
- The only `holds-r` record was taken while held. So that path can only be fed a synthetic pair.
- Fed `release-2` with that held `holds-r`, it reads "kept the hold", correctly for that input but not the real case (`readings-*.txt`, `release …`).
- Add `holds-r-released` after `release-1`.

**G2. "Snapshot gone" is recorded only through a pool that does not exist.** Certainty: high that it is missing; medium on ZFS's path.

- On `skznone`, `zfs hold` and `zfs release` fail at `zfs_open` of the parent ("cannot open 'skznone': dataset does not exist").
- On a live pool with no `@t`, libzfs takes another path (`zfs_hold` and `zfs_release` with an empty list). Its message is not recorded.
- No `zfs holds -H`, `hold` or `release` on `skzq@absent` is recorded.
- These are the inputs of `held → false` and `release → Ok` (`cli.rs:338, 345, 376`), which #147 lists as resting on real ZFS.
- Add `holds-absent`, `hold-absent` and `release-absent` on each live pool.

**G3. Several readings #147 lists are not exercised:**

- a rebuild, which a sequential `zpool attach -s` would give;
- a removal in progress (`zpool remove`);
- the class headers `cache`, `spares`, `special` and `dedup` (only `logs` appears);
- `zfs.list.user_properties` and "set locally" (`snapshot.lua` stops at line 42);
- the snapshot program's success shape and `createtxg`;
- the values beyond ASCII in its JSON;
- a checkpoint of `0`.

The middle three wait on #147's fix. The first three can be had cheaply on the mirror pool (files).

**G4. Ubuntu 24.04 is not ZFS 2.2.** Certainty: high (the `version` and `about.txt` records).

- The 24.04 runner had kernel 7.0 with the `zfs-kmod-2.4.1` module under the `zfs-2.2.2` userland. The channel programs run in the module.
- So no 2.2 Lua sandbox is recorded. #147's docs expect "recorded output of ZFS 2.2 (Ubuntu 24.04)".
- What the 24.04 records do show is the 2.2.2 userland's messages and JSON. That is how the `zpool get` difference below appeared.
- The description states the pair plainly. It should also say that a 2.2 module (a stock 24.04 kernel) is not covered.

**G5. A partial hold is not recorded (for #147's reviewer).** Certainty: medium (from libzfs and `dsl_userhold.c` as I recall them).

- The adapter counts "tag already exists" as held (`cli.rs:330`).
- With `-r`, the hold is one sync task, and its check stops at the first error other than ENOENT. So if only the root's `@t` has the tag, the hold most likely adds none to the children and still says "tag already exists" for the root.
- A record would settle it: `zfs hold skyzfs-publish p@t` without `-r`, then `zfs hold -r`, then `zfs holds -H -r`. The same goes for a `zfs release -r` where some members lack the tag (libzfs most likely exits 0).

**G6. The unit tests do not pin the command lines that matter.** Certainty: high (plants).

- `test_zfs_cli_record.sh` tests `rec`, `catch`, `keep` and `program` (now with `--locale`), but not `agent_commands` or `record_pool`.
- Each plant was applied alone to a copy of the head's script, then the PR's unit test was run (`review/q10/plants.sh`; outputs `plants.out` at `bd37422`, `plants-73dce38.out` at `73dce38`).
- At `73dce38`: 17 plants, 9 caught, 8 survived. The results at `bd37422` were the same for U1 to U15.

| Plant | Result |
|---|---|
| U1: `-j` dropped from `program` | caught |
| U2: memory limit `100000000` | caught |
| U3: `rec` without stdin | caught |
| U4: `rec` without its limit | caught |
| U5: `catch` without its limit | **survives** |
| U6: `catch` reads stderr | caught |
| U7: `catch` keeps the first try's rc | **survives** |
| U8: `keep` takes a name twice | **survives** |
| U9: `zpool status` without `-P` | **survives** |
| U10: `zpool get` without `-p` | **survives** |
| U11: `holds -r` without `-H` | **survives** |
| U12: hold tag changed | **survives** |
| U13: `members.lua` without `-n` | **survives** |
| U14: not-caught note says caught | caught |
| U15: `catch` returns 0 when not caught | caught |
| U16: `--locale` read and dropped | caught |
| U17: `--locale` sets `LANG` | caught |

- U9 to U13 are the fidelity the recorder exists for, and D1 is exactly such a change.
- Suggest: a unit test that runs `agent_commands` and `record_pool` on fake `zpool` and `zfs` commands (on `PATH`, as the new `--locale` test does) and compares each `cmd` with the adapter's words, written out in the test.
- Better still, in #147's fixture follow-up, assert that each fixture's `cmd` equals the words the adapter ran on the fake host. That pins the two together whichever side moves.

### Suggestions

- **S1. Persisted credentials.** `actions/checkout@v6` persists the token (`yml:54`).
  - The repository is public, so `git fetch origin "$LUA_REF"` needs none.
  - Meanwhile the Record step runs, as root, a build of third-party code (`build-ublksrv.sh`) and Lua from any ref.
  - `persist-credentials: false` costs nothing. The token is `contents: read`, so this is hygiene.
- **S2. `matrix.image` in `run:` scripts.** `${{ matrix.image }}` is interpolated into them at `yml:78` and `:81`.
  - The value comes from the dispatch input `images`.
  - It is not exploitable in practice: `runs-on` must match a runner label before any step runs, and dispatch needs write access.
  - It is the pattern the rest of the file avoids for `lua_ref`. Pass it through `env:`.
- **S3. `lua_ref` resolution.** The ref is resolved once per image job, so a branch that moves between the two jobs records two different commits.
  - Each `about.txt` names its own commit, so this is visible. Resolving once in a small job and passing the SHA would make it impossible.
  - Also, the default stays 366b217 after #147 moves on. The recorder's command lines are fixed in the script while the Lua comes from `lua_ref`, so the two can drift apart with nothing to say so (see G6).
- **S4. A hung status.** `catch` spends `LIMIT` (120 s) on each of 100 tries when a status hangs: about 3 h 20 min, against the job's 40 minutes.
  - Plant P5: 3 tries at `LIMIT=1` took 5 s.
  - Stop the loop on rc 124.
- **S5. The shell's kill message in a record.** When `lim` kills a command, the shell's "Killed" line, with the script's own path, lands in that record's `stderr` (P5). A fixture author should know that line is not ZFS's.
- **S6. A name used twice in `catch`.** `catch` ignores `keep`'s refusal of a name used twice. It returns 0 when caught and leaves `OUT/.catch` behind until the next `catch` (P6). The names are fixed in the script, so this is latent.
- **S7. Escaped bytes in `cmd`.** `printf %q` escapes commas (`property\,value`), and since `73dce38` a non-UTF-8 argument is written as `$'…\377…'`. A fixture test that compares words must unquote. Storing the argument vector NUL-separated beside `cmd` would avoid that.
- **S8. No timeout on `unit`.** The `unit` job has no `timeout-minutes`.

## 1. Is each record what the adapter would see?

The adapter's runner (`host.rs:78-148`) and `ZfsCli` (`cli.rs:152-206`), against `rec`, `catch` and `program` (`zfs-cli-record.sh:94-172`):

| | Adapter at 366b217 | Recorder at 73dce38 | Same? |
|---|---|---|---|
| `zpool status` | `-P POOL`; `-P -g POOL` | same | yes (D6: some states lack `-g`) |
| `zpool get` | `-H -p -o property,value guid,load_guid,multihost,readonly,checkpoint POOL` | same | yes |
| snapshot program | `zfs program -j -t 100000000 -m 104857600 POOL - POOL T skyzfs:assist` | same | yes |
| members program | `zfs program -n -j … POOL - POOL T` | same | yes |
| tagged program | `zfs program -n -j … POOL - POOL skyzfs-publish` | `… POOL skyzfs` | **no (D1)** |
| `zfs hold`, `zfs release` | `-r skyzfs-publish POOL@T` | `-r skyzfs POOL@T` | **no (D1)** |
| `zfs holds` | `-H POOL@T`, `-H -r POOL@T` | same words; output names `skyzfs` | words yes, output **no (D1)** |
| assist | `zfs set skyzfs:assist=V POOL`; `zpool sync POOL` | not recorded | **no (D2)** |
| `nocacheflush` | read of the module parameter's file | `cat` of the same file | yes |
| environment | `LC_ALL=C`, plus the agent's own | `C.UTF-8` through sudo; `LC_ALL=C` only for `snapshot-odd-c` | **no (D5)** |
| stdin | `/dev/null` when empty; a pipe with the Lua for programs | `/dev/null`; the `.lua` file redirected | equivalent: `zfs program -` reads fd 0 to EOF either way |
| working directory | the agent's | the workspace | no effect: every name is absolute |
| time limit | read 60 s, change 120 s; killed past it | 120 s for every command; 124 past it | does not change output; a 60-120 s command would pass here and fail there |
| captured | exit code (`None` on a signal), stdout, stderr | rc (128+n on a signal), stdout, stderr, always all three | yes |

### The recordings against the adapter's readings

Each record was fed, as data, to `ZfsCli` at 366b217 on its own fake host, under the words the adapter runs. The scratch test is `review/q10/q10_scratch.rs`; its results are in `readings-ubuntu-24.04.txt`, `readings-ubuntu-26.04.txt` and `readings-ubuntu-26.04-73dce38.txt`.

| Reading (#147 lists it as resting on real ZFS) | 24.04 (2.2.2 userland, 2.4.1 module) | 26.04 (2.4.1) |
|---|---|---|
| `zfs program` reads a script named `-` from stdin | confirms (members and tagged ran; `snapshot.lua` ran to line 42) | confirms |
| Output shape `{"return": …}`; an empty table | confirms; empty table is `{}` (an object) | confirms; `{}` |
| GUIDs as signed strings, read modulo 2^64 | confirms (negative strings, e.g. `-1093669918614105072`) | confirms |
| `members` with `@t` absent → `None` | confirms (`{"absent":"1"}`) | confirms |
| `tagged` before and after a hold | confirms (`{}`, then `{"t":"1"}`) | confirms |
| `zfs.list.system_properties` and `zfs.list.user_properties`; "set locally"; values beyond ASCII | **not exercised**: `pcall` is nil in the sandbox ("attempt to call global 'pcall' (a nil value)", exit 1). `system_properties` was entered, `user_properties` never | same, in all three locales at 73dce38 |
| The snapshot program's success shape and `createtxg` | **not exercised** | same |
| EEXIST → `Exists` | not recorded | **confirms at 73dce38** (`{"failed":"skzq/child@p","errno":"17"}`, exit 0 → `Err(Exists)`, nothing taken) |
| A program on an absent pool → `NoPool` | confirms ("cannot open 'skznone': no such pool") | confirms |
| Checkpoint is none where `-` or `0` | `-` → false confirms; `184320` → true confirms; `0` not seen | same |
| A scrub under way is not busy | confirms (in progress, paused, canceled, done) | confirms |
| A resilver under way is busy | confirms (rig and mirror) | confirms on rig; mirror not caught in either run (noted) |
| A rebuild, a removal | **not exercised** (G3) | same |
| Layout: `-P` and `-g` of one shape, `logs` header, `mirror-N` with its GUID from `-g`, file leaves | confirms | confirms |
| `zfs holds -H`: `name<TAB>tag<TAB>date` | confirms (format); tag wrong (D1) | same |
| `zfs holds -H -r`: one line a member's `@t` | confirms while held; after release not recorded (G1) | same |
| "tag already exists" counts as held | confirms (exit 1; names the root's `@t` only) | confirms |
| "no such tag" | confirms ("cannot release hold from snapshot 'skzq@t': no such tag on this dataset", exit 1) | confirms |
| `zpool status` on an absent pool → `NoPool` | confirms | confirms |
| `zpool get` on an absent pool → `NoPool` | **contradicts**: exit 2, "missing pool name." and the usage, read as `Other`. The engine's `sweep` returns early only on `NoPool` (`engine.rs:493-497`), so on this userland it errors instead | confirms ("Cannot get properties of skznone: no such pool available.", exit 1) |
| `holds`, `hold`, `release` on an absent pool | "dataset does not exist", so `held` → false, `hold` → `Err(NoSnapshot)`, `release` → `Ok`: an absent pool reads as an absent snapshot, never `NoPool`. Confirms the message; whether that mapping is wanted is #147's call | same |
| A snapshot absent on a live pool (`holds`, `hold`, `release`) | **not recorded** (G2) | same |
| `nocacheflush` | `0` → false confirms | same |

The other 26.04 readings at `73dce38` have the same shape as at `bd37422`.

## 2. `catch`'s not-caught path, planted

`review/q10/catch_probe.sh` sources the head's script with fake commands and no kernel. Its output is in `catch_probe.out`. These functions did not change in `73dce38`.

| Plant | What was kept | Return |
|---|---|---|
| P2: a status that fails every time | the last try: rc 1, its stderr, note "not caught: no output of 3 matched /…/; this is the last", in the index and the summary | 1 |
| P3: earlier tries succeed, the last fails | the last try's rc and output, with the note | 1 |
| P4: the state shows on stdout of a command that exits 2 | "caught at try 1", rc 2 kept | 0 |
| P5: a command that never ends | rc 124 with the note; each try costs `LIMIT` (S4); the shell's kill message lands in `stderr` (S5) | 1 |
| P6: a name used twice | `keep` refuses (70) and the first record stays; `catch` returns 0 and `OUT/.catch` is left (S6) | 0 |
| P1 (`rec`, for contrast): an expected failure that succeeds | rc 0, no note, nothing flags it (D4) | — |

So `catch`'s not-caught path is recorded as such and flagged, and nothing falls over. The gap is in `rec` (D4).

## 3. The workflow (`.github/workflows/skyzfs-zfs-cli-record.yml`, the same in both commits)

**Injection.**
- `lua_ref` reaches the script only through `env:` (`:61`), never through `${{ }}` in `run:`.
- The `case` at `:63-65` refuses:
  - an empty value;
  - a leading `-`, so no option can reach `git fetch`;
  - any character outside `[A-Za-z0-9._/-]`, so no `:` destination, `+`, glob, `^`, `~` or `@` can make a refspec of it.
- That is strict enough. What passes is one source ref or a full SHA. `..` passes the `case` and is then refused by git's own ref-name check.
- An abbreviated SHA fails loudly, since git cannot fetch it by name. The input's description says "commit", so say "a full commit".
- `refs/pull/N/head` passes, which is useful.
- `matrix.image` is interpolated into `run:` (S2).

**Check against fetch.**
- The `case` checks a string. What is fetched is whatever that name resolves to at fetch time.
- The step then reads the files and the recorded `LUA_REF` from the same `FETCH_HEAD` (`:69-71`), so within a job the commit named in `about.txt` is the one whose Lua ran.
- Across the two image jobs, a moving branch can differ (S3).
- A name that is both a tag and a branch resolves by git's rules (the tag first). The SHA recorded says which.

**Permissions.**
- `permissions: contents: read` at the top.
- No secrets.
- The checkout persists the token (S1).
- Actions are pinned by tag (`@v6`), as everywhere else in the repository.

**apt-fallback.** It runs right after checkout in `record`, the one job that installs packages. `unit` installs none.

**Dispatch only.**
- The `record` job is gated by `if: github.event_name == 'workflow_dispatch'`.
- A push to `skyzfs-testing*` touching the three files runs only `unit`, as the description says. It is there so that the workflow can be dispatched from that branch.
- There is no `pull_request` trigger. On a pull request, the unit test runs in `ci.yml`'s lint job.

**Concurrency.** Two dispatches on one ref queue; they do not cancel each other.

**Shellcheck.** Clean in CI on both heads: job `lint`, step `shellcheck`, in `ci` runs 45653670702 and 45656198791.

## 4. Tests, gates, CI

**Local, at `73dce38`.**
- `bash tests/kernel/test_zfs_cli_record.sh`: 29 of 29 ok, exit 0. At `bd37422` it was 26 of 26.
- `bash -n tests/kernel/zfs-cli-record.sh`: ok.
- The workflow parses as YAML; its triggers are `workflow_dispatch`, and `push` on `skyzfs-testing*` for the three paths.
- The local shellcheck is 0.9.0, older than CI's: it reports SC2317 notes across the whole repository, so it is not a gate here. I relied on CI's shellcheck.
- No Rust is touched, so no cargo gate applies.
- The scratch adapter test built and ran with `cargo test -p skyzfs-agent --lib q10_scratch` on a scratch checkout of 366b217 (toolchain 1.100.0).

**Plants on the unit test.** 17 plants, 9 caught, 8 survived (G6).

**CI on `73dce38`, at the time of writing:**

| Run | State |
|---|---|
| `skyzfs-zfs-cli-record`, dispatch 45656199148 (26.04) | green (judged above) |
| its push run 45656181404 | green |
| `skyzfs-ci` push run 45656181327 | green |
| `ci` 45656198791, job `lint` (shellcheck and the unit test) | green; other jobs still running |
| `skyzfs-kernel-probe` 45656198814 | green |
| `e2e` 45656198698 | green |
| `coverage` 45656198795 | running |
| `skyzfs-daemon-rig` 45656198746 | running |
| `skyzfs-replay-rig` 45656198598 | running |

- The rigs run because the PR touches `tests/kernel/`. They test nothing this PR changes, but the lead reads their tables.
- On `bd37422`, the rigs were cancelled by the newer push. Lint and the kernel probe were green there too.

**Codex.** Codex posted only a usage-limit notice, with no findings to answer.

## Files beside this note

- `q10_scratch.rs`: the adapter at 366b217 fed the records. Its header says how to run it.
- `readings-ubuntu-24.04.txt`, `readings-ubuntu-26.04.txt` (run 45652653655) and `readings-ubuntu-26.04-73dce38.txt` (run 45656199148): its output.
- `plants.sh`: the plants on the unit test (`bash review/q10/plants.sh HEAD WORKDIR`). Its output is `plants.out` at `bd37422` (U16 and U17 do not apply there) and `plants-73dce38.out`.
- `catch_probe.sh`, `catch_probe.out`: the probes of `rec` and `catch` (`bash review/q10/catch_probe.sh HEAD WORKDIR`, run at `bd37422`; the functions are unchanged at `73dce38`).
