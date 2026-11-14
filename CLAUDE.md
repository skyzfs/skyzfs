# SkyZFS: guide for Claude sessions

This repository began as a snapshot of SkyNBD. It is now the home of
SkyZFS: OpenZFS on a block device served by a node-local Rust daemon,
with native clients on Linux, macOS and Windows as the goal.

Read these before changing anything:

- `docs/skyzfs-design.md`: architecture, roadmap, decisions. The current
  milestone and its exit criteria are in §9.
- `docs/skyzfs-impl-notes.md`: by milestone and work package, the design
  as built, what the rigs and the benchmark showed, and the review
  record. A section there carries the number of the design section it
  details, so "design §4.2, The states of a device" is found under
  "The ublk transport (design §4.2)".
- `docs/skynbd-postgres-study.md` §10: the measurements behind the design.
- `CONTRIBUTING.md`: the test-first workflow and the ground rules
  (`cargo fmt`, `cargo clippy -D warnings`, no `unwrap()` outside tests,
  versioned formats). They apply to all new code.

## Layout

- `crates/skynbd-*`, `bins/skynbd*`: the existing engine, object formats,
  refs, NBD server and dm-thin agent. Kept. See design §3.1 for what
  changes.
- New code goes in crates named `skyzfs-*` and in `bins/skyzfsd`.
- `tests/bench/topology-bench.sh` and
  `.github/workflows/topology-bench.yml`: the I/O benchmark.

## Sessions and branches

Four long-lived sessions work on this project. Each pushes only to its
own branch, and every work package reaches `zfs-topology` through a pull
request.

| Session | Role | Branch |
|---|---|---|
| skyzfs lead | Plans work packages, merges pull requests, owns `docs/skyzfs-design.md` and `docs/skyzfs-impl-notes.md` | `zfs-topology` (integration) |
| skyzfs coding | Implements work packages | `skyzfs-coding` |
| skyzfs testing | CI, test rigs and benchmarks, and running them | `skyzfs-testing` |
| skyzfs review | Reviews pull requests and posts its findings on them. Does not change product code | none |

- The user may join any session and redirect it. The user's instructions
  take precedence over a work package. Tell the lead what changed.

## One pull request per work package

The user asked for this: a pull request is reviewed automatically by
Codex, and other people can take part.

1. **Start** from the tip of `origin/zfs-topology`. If your last pull
   request has merged, restart your branch there:
   `git fetch origin zfs-topology && git checkout -B <branch> origin/zfs-topology`,
   then push with `--force-with-lease`. Never stack new work on merged
   history.
2. **Open the pull request yourself** when the work package is ready
   for review: your branch into `zfs-topology`, never into `main`. The
   title starts with the work package id (`M1.1: ...`). The description
   follows `.github/pull_request_template.md`. GraphQL is not available
   in cloud sessions, so use the REST API:
   `gh api repos/skyzfs/skyzfs/pulls -f title=... -f head=<branch> -f base=zfs-topology -F body=@file`.
   Then ask the review session for a review, with the link, the head
   and what you want judged in particular, and send the lead a digest.
3. **Review.** Codex reviews on its own. The review session posts its
   findings on the pull request itself, most severe first, says which
   ones block the merge, and sends the author and the lead a digest. The
   author answers every finding on the pull request, from whoever it
   comes: fix it with a new commit, or say why not; answer Codex on its
   thread and resolve what is settled; then ask the reviewer for the
   next look. Check a finding against the code before acting on it, a
   bot's above all. Do not force-push a branch that is under review.
4. **Merge.** When the review says it can merge, CI is green on the
   head and every blocking finding is settled, the author sends the lead
   a digest that says so. The lead reads the rigs' tables and merges,
   with a merge commit. The author does not merge their own pull
   request. A finding that needs a decision (the design, scope, what a
   criterion means) goes to the lead when it comes up, not at the end.
5. **One open pull request per branch.** If the next work package must
   start before the last has merged, branch off as `<branch>-<package>`
   (for example `skyzfs-coding-m1.2`) and tell the lead if your session
   cannot push it.

The lead's own changes to the design document, the implementation
notes and this guide go straight to `zfs-topology`.

### Between sessions: digests and pointers

The pull request is the record. A message to another session points at
it and does not repeat it.

- **A digest is a few lines:** the pull request's link and head, what
  state it is in (opened, reviewed with or without blocking findings,
  answered, ready to merge, blocked), and what needs a decision from
  the one you write to. No pasted description, review or log; give the
  link to the comment or the run.
- **Who is told what.** The author tells the reviewer when there is
  something to look at, and the reviewer tells the author the verdict.
  The lead gets a digest when a pull request opens, when a review has a
  blocking finding or a question of design, and when it is ready to
  merge. Measurements that the design rests on (a benchmark round, a
  rig's first run on something new) still go to the lead in full, as
  figures, since the lead writes them into the notes.
- **Do not subscribe to a pull request's events.** Every event wakes
  the session and makes it read its whole context again, and most
  events are a bot editing its status line. Wait for CI with a
  background loop on the head's runs, and read Codex's findings once
  when CI is done and once more before you say "ready to merge".
- The lead does not post for other sessions and does not pass findings
  on. If a session may not open pull requests or post on GitHub by its
  own standing instructions, it says so once and waits for the user.

The design is also open for outside review as a documents-only pull
request into `main`, from the branch `skyzfs-design-docs`
(skyzfs/skyzfs#2). The lead copies `docs/skyzfs-design.md`,
`docs/skyzfs-impl-notes.md` and `docs/skynbd-postgres-study.md` to that
branch whenever they change and answers its comments. Nobody else pushes
there, and the user decides when it merges.

## Easy to miss

- **No kernel features in cloud containers.** There is no ublk, ZFS,
  device-mapper or loop device here. Anything that needs one runs in
  GitHub Actions. Pure Rust builds and tests run locally.
- **The workspace needs three system packages to build**, two because
  of `skynbd-agent` and libclang because of the ublk binding
  (`skyzfs-ublk-sys`):
  `apt-get update && apt-get install -y --no-install-recommends libdevmapper-dev libudev-dev libclang-dev`.
- **Two cargo features hold code that the workspace's tests do not
  build.** `skyzfsd --features test-hooks` has the daemon's fault hooks
  (the rigs use that build), and
  `skyzfs-ublk-sys --features control-test-program` has the test
  program `skyzfs-ublk-control`, its parse tests and the binding's
  kernel tests. Run clippy and the tests with each as well as plain.
  The kernel tests are ignored tests (root and the ublk driver); the
  control rig is the workflow that runs them.
- **Run `rustup update stable` before the local gates.** CI installs the
  current stable toolchain, and a container's can be months older. A
  newer clippy has lints an older one does not, so "clean locally" on
  an old toolchain is not clean in CI.
- **CI coverage.** `ci.yml`, `coverage.yml` and `e2e.yml` run on `main`
  and on every pull request, whatever its base. `skyzfs-ci.yml` runs
  shellcheck, fmt, clippy and the workspace tests on the pushes those
  miss: a merge into `zfs-topology`, and a push to a `skyzfs-*` branch
  that has no pull request open. On a `skyzfs-*` branch a newer push
  cancels the run of the older one, and a cold build takes over ten
  minutes, so let a run finish before pushing again when you need it
  green.
- **Package mirrors.** Every job that installs packages calls
  `bash tests/lib/apt-fallback.sh` right after checkout; a new job that
  uses apt needs that step too. When a name of the runner image's Ubuntu
  mirrors does not resolve, the script replaces the mirror list by one
  mirror outside `ubuntu.com` (named in the script) and says so in the
  job's summary; otherwise it changes nothing, and it does nothing on a
  runner that is not GitHub-hosted. apt's signature checks are not
  touched. `skyzfs-mirror-canary.yml` (dispatch only; input `force`
  takes the fallback path on a healthy day) tries it for real. A job
  that still fails on an unresolved name is an outage, not the change:
  report it and do not rerun blind. On an arm64 image the fallback is a
  mirror of `ubuntu-ports`. `ubuntu-24.04-arm` has no mirror list, so
  it has no fallback, and its job summary says "no fallback on this
  image": a failure on an unresolved name there is an outage.
- **arm64.** amd64 and arm64 are both supported (design goal 8). The
  job `unit-arm64` in `ci.yml` and `skyzfs-ci.yml` runs clippy and the
  tests natively on `ubuntu-26.04-arm`, plain and with each of the two
  features below, on every pull request. The control rig and the daemon
  rig (the daemon only, not the stand-in) also run on `ubuntu-26.04-arm`
  on a pull request that triggers them. `ubuntu-24.04-arm`, the
  stand-in on arm64 and the replay rig on arm64 run by dispatch (the
  rigs' `images` inputs). The hosted arm64 images have
  4 KiB pages only: nothing here shows 16 KiB or 64 KiB pages on a real
  kernel.
- **The kernel probe.** `skyzfs-kernel-probe.yml` runs on a pull request
  that touches `tests/kernel/` or the workflow, and by dispatch. It
  fails when a property the design depends on does not hold on the
  runner (`pin_*` lines in its summary).
- **The replay rig.** `skyzfs-replay-rig.yml` runs on a pull request
  that touches `tests/kernel/`, `tests/lib/`, the workflow or the
  daemon's code (below), and by dispatch. It has two kernel jobs,
  one per pool layout, of about twenty and twenty-six minutes. It
  records a pool's writes and checks the pool at every record prefix,
  every copy of every reachable block at the first and last prefix of
  each transaction group, and what `fsync` promised. It also fails when
  a negative control's result differs from the table `EXPECTED` in
  `tests/kernel/replay_check.py`, in either direction: changing what the
  rig catches means changing that table on purpose. A newer push to a
  pull request cancels the older run. The dispatch input `discard`
  chooses whether a discarded range is zeroed or left as it was.
  The daemon has one store, the in-place store (the log store was
  removed in M1.3t), and since M1.T31 no rig and not the benchmark
  names a store; a current build has no `--store` at all
  (M1.3t-d), so any `--store` in `serve_args` is refused, as the rig's own error before
  any serve (M1.T34). A build from before M1.3t (a `daemon_ref` or
  `TB_SKYZFSD_REF`) defaults to the log store, so when its help offers
  `[--store` it is served with `--store in-place`, and the summary
  says so.
  Since M1.T14 the workflow also has the jobs "through skyzfsd", one
  per image (`tests/kernel/replay_daemon.py`): the cut is below the
  daemon, on the disk under its files, and each prefix is checked
  through a daemon started as after a reboot. A pull request that
  touches `tests/kernel/`, `tests/lib/` or the workflow runs both kinds
  of job; one that touches `crates/skyzfs-local/`, `crates/skyzfs-ublk/`,
  `crates/skyzfs-ublk-sys/` or `bins/skyzfsd/` runs the daemon jobs.
  On a pull request they check the marks and every flush boundary,
  about twenty-five minutes a job. By dispatch, `server` chooses
  `reference` (the default), `daemon` or `both`, and `daemon_budget`
  `full` (the default: every record prefix with no time limit; 34
  and 39 minutes a job in its first run) or `pull-request`. The
  summary's line "Every record prefix: N of N" says whether a run
  shows design §9's criterion; only a full run that checked N of N
  and ended with exit 0 does. Exit 1 is a verdict against the daemon,
  exit 2 the rig's own error. The jobs "write into a hole" (M1.T27,
  `tests/kernel/hole-rig.sh`) run wherever the daemon jobs run, and
  alone by dispatch with `server: hole`: direct writes into blocks the
  image never held, on ext4 and XFS, on a sparse, a preallocated and a
  written-through (`create --zero-fill`) image. Every checked prefix
  must be as design §4.1 allows, the control `withheld` must be
  caught, and on a written-through image the count of prefixes with
  the data on the disk and zeros in the file must be 0; the other
  counts are measurements. The input `daemon_ref` builds the daemon of
  the daemon and hole jobs from another commit.
- **The daemon rig.** `skyzfs-daemon-rig.yml` runs on a pull request
  that touches `tests/kernel/`, `tests/lib/`, `bins/skyzfsd/`,
  `bins/skyzfs-blkcheck/`, `crates/skyzfs-ublk/`,
  `crates/skyzfs-ublk-sys/`, `crates/skyzfs-local/`, `crates/skyzfs-blk/`,
  `Cargo.lock`, `Cargo.toml` or `rust-toolchain.toml`, and by dispatch.
  It drives a server through the kernel and ZFS on Ubuntu 24.04 and
  26.04: parameters, overlapping writes, `kill -9` under load, a start
  that is refused, a device that waits 300 s for a server, the
  conformance suite through the block device, a failed volume, a held
  request. **A pull request runs it on `skyzfsd`** (built with its test
  hooks), about thirty-five minutes per image. One that changes
  `tests/kernel/`, `tests/lib/` or the workflow also runs it on the
  stand-in (`tests/kernel/skyzfsd-ref.sh`, about thirty-three minutes),
  which is what shows that the rig still catches what it caught. A push
  of the workflow file on a testing branch runs the stand-in. By
  dispatch the input `server` chooses `daemon` (the default), `stand-in`
  or `both`, `checks` the checks (for example `c` alone). Two lines
  measure the reference server's count of writes applied and not
  acknowledged and read NA on the daemon
  (`NOT_ON_THE_DAEMON` in `tests/kernel/daemon_check.py`); the judge
  fails on any other NA. The log store's lines were removed in M1.T31. The job
  `which server` says in its summary what was chosen, and artifacts are
  named `daemon-rig-<image>-<server>`. **The jobs build the pull
  request merged with its base as the base was when the pull request
  was last pushed**: after another pull request has merged, the daemon
  they drove is not the one on `zfs-topology`, and the rig is dispatched
  there after the merge. A green run is not a verdict on a
  failure with a request held: that one pin is WAITING, and the
  summary says so. Check g (M1.T23) puts the volume's
  directory on a device-mapper device that the rig switches to the
  `error` target, so a sync of the home image fails in the kernel: it
  pins that the next start serves what the disk holds and not what
  only the cache held, and what the daemon and the pool do when the
  disk fails at a start or while serving. Its negative control is a
  daemon from before the cache drop, by dispatch: `daemon_ref` with
  cd0e763's full id and `checks: g`, where five
  lines must differ (runs 43541748747 and 43555427037), the daemon's failure reason among them
  (`pin_failure_reason_is_the_images`, M1.T26: the reason the daemon
  gives for a failed image is read in four places). A start that
  the daemon refuses after a `kill -9` is a FAIL of the kill test's
  pins, not an error of the rig (M1.T31), and so is a daemon that
  ends with status 7, a request handed over twice by the driver
  (M1.3ac, M1.T34) (status 8, a commit the driver refused, is to be
  read the same way from M1.T35); a refused start may change
  only the status file. Check f was removed with the log store. The summary
  says for each line whether it rests on the kernel, on the server, or
  only shows that the check works. Exit 2 is an error of the rig and
  not a verdict on the server. Read the summary's table and not the
  colour. When check c fails, download the run's `c-trace.tsv.gz` and
  run `skyzfs-blkcheck --replay c-trace.tsv VOLUME`: it replays that
  volume's calls through the model with no kernel and prints the same
  violation, or shows that the rig's bookkeeping and not the server was
  at fault. A replay that agrees with the rig does not show the server
  wrong: the trace holds the tool's calls, not the requests the server
  was handed, and a call can return long after the kernel completed it
  (in #58 a udev read held a page of a direct write's range across a
  kill, and the tool took the write for reissued). Since M1.T28
  check c runs with udev's watch on ublk devices off, and the artifact
  of a pass where a case failed keeps what the server was handed and
  answered (`c-cN.requests.tsv.gz`, `cd-` for the pass with the delay,
  from `serve --request-log` of a test-hooks build that has it) and the
  kernel's block trace on the same clock (`c-ftrace.txt.gz`): read the
  volume from both sides before calling such a failure a verdict. The
  line `obs_c_evidence` says what was kept; a pass with no block trace
  is an error of the rig. `skyzfs-c-repeat.yml` (dispatch) repeats
  check c's passes on one runner; its input `provoke` turns the watch
  back on and keeps buffered reads of each device going, which is how
  the late return was made on purpose, and `blkcheck_ref` builds the
  tool and its model from another commit under the same daemon.
- **The control rig.** `skyzfs-control-rig.yml` runs on a pull request
  that touches `crates/skyzfs-ublk-sys/`, `crates/skyzfs-ublk/`,
  `crates/skyzfs-blk/`, `crates/skyzfs-local/`, `Cargo.lock`,
  `Cargo.toml`, `rust-toolchain.toml`, `tests/lib/` or the rig's own
  files, and by dispatch; under three minutes per image. It runs the
  binding's kernel tests and then takes the binding through the
  driver's states with its test program, beside devices of the
  reference server. It fails when a line differs from the table
  `EXPECTED` in `tests/kernel/control_check.py`, in either direction. A
  kernel test that hangs or panics is a verdict on the binding (exit
  1), not an error of the rig.
- **A pool whose device was deleted goes on reading ONLINE while it
  only writes**, and posts tens of thousands of events a second. A tool
  that reads `zpool events` from it does not come to an end: cut such a
  read with `head` and a limit. A writer or reader on a pool whose
  device has no server does not end when told to: never wait for one
  without a limit (`job_end` in `tests/kernel/daemon-rig.sh`).
- **Failed-volume observations.** `skyzfs-failed-volume.yml` runs by
  dispatch only. It records what a pool does when its device is deleted
  under it and judges nothing.
- **A command on a suspended pool may never return and cannot be
  killed**, and `timeout(1)` waits for such a child. Scripts that touch a
  pool after a failure use `lim` from `tests/lib/lim.sh`, with the
  command's output in files and not in a command substitution.
- **The benchmark runs by dispatch only** on `zfs-topology` and
  `skyzfs-*` branches: dispatch `topology-bench.yml` with the branch as
  the ref. A push no longer starts it there. On any other branch but
  `main`, a push that touches `topology-bench.sh`, `tests/lib/gnu-dd.sh`
  or the workflow still starts thirteen runner jobs.
- **The daemon in the benchmark.** The topology `zfs-skyzfsd` serves
  the pool's data device with a release build of `skyzfsd`. Its summary
  judges a daemon row against the model (`zfs-slog`) by the mean of
  the two model rows that enclose it in the sequence (M1.T12): put a
  model row at least every third row, first and last. A row gets a
  verdict only when both model rows ran on the same machine with its
  clients, duration and `TB_MEM_LIMIT`, had no origin delay
  (`origin_delay_ms: 0`; the workflow's default is 10), and differ by
  at most 10% of their mean. Anything else ends with NO VERDICT and
  says why, which is not a failure of the run. The criterion itself is
  a median over rows and jobs (design §9), not one row's verdict.
  Since M1.T11 an entry may name `TB_SKYZFSD_REF=<commit or branch>`
  and runs that build of the daemon, so several builds are compared in
  one job; `TB_SKZ_TIME_REF=<ref>` reads the job's times with that
  ref's `skz_time.py` (name the newest daemon's). Each job starts with
  a disk-sync line that says whether the machine's syncs are slow or
  fast, and the work disk's mount options and write cache.
  `TB_SYSCALLS=1` on an entry traces the daemon's system calls and
  `TB_PERF_RECORD=1` records a profile of it (M1.T15); a row that was
  traced or recorded counts for no ratio.
  `TB_SKYZFSD_MEM` starts the daemon in a memory cgroup of its own, and
  postgres runs in a cgroup of its own in every row; rows from before
  M1.T12 are not known to be comparable in tps with rows after it. The
  script records and exits 0 whatever the ratio.
- **Runner noise.** Two GitHub runner VMs differ by more than most
  effects being measured. Compare configurations only on one runner (the
  workflow's `sequence` input), and say how many runs a number rests on.
- **Do not weaken a test to make it pass.** Report it.
- **Commit messages** say what changed and why, in plain words. No model
  names and no session identifiers.

## Reporting a work package

The pull request's description is the report. It has:

1. the work package and the commit range,
2. what was done against each acceptance criterion,
3. how it was verified: the commands and their results, and links to any
   CI runs,
4. what is not done, and what you are unsure of.

Send the lead a message with the link and anything that needs a
decision. A reviewer who is not on this project reads the description
cold, so write it for them.
