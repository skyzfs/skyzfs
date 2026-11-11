# SkyZFS: guide for Claude sessions

This repository began as a snapshot of SkyNBD. It is now the home of
SkyZFS: OpenZFS on a block device served by a node-local Rust daemon,
with native clients on Linux, macOS and Windows as the goal.

Read these before changing anything:

- `docs/skyzfs-design.md`: architecture, roadmap, decisions. The current
  milestone and its exit criteria are in §9.
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
| skyzfs lead | Plans work packages, merges pull requests, owns `docs/skyzfs-design.md` | `zfs-topology` (integration) |
| skyzfs coding | Implements work packages | `skyzfs-coding` |
| skyzfs testing | CI, test rigs and benchmarks, and running them | `skyzfs-testing` |
| skyzfs review | Reviews pull requests. Does not change product code | none |

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
2. **Open the pull request** when the work package is ready for review:
   your branch into `zfs-topology`, never into `main`. The title starts
   with the work package id (`M1.1: ...`). The description follows
   `.github/pull_request_template.md`. Then send the lead the link.
   GraphQL is not available in cloud sessions, so use the REST API:
   `gh api repos/skyzfs/skyzfs/pulls -f title=... -f head=<branch> -f base=zfs-topology -F body=@file`.
3. **Review.** Codex reviews on its own. The review session posts its
   findings on the pull request, most severe first, and says which ones
   block the merge. Answer every finding on the pull request, from
   whoever it comes: fix it with a new commit, or say why not. Do not
   force-push a branch that is under review.
4. **Merge.** The lead merges, with a merge commit, once CI is green and
   every blocking finding is settled. The author does not merge their
   own pull request.
5. **One open pull request per branch.** If the next work package must
   start before the last has merged, branch off as `<branch>-<package>`
   (for example `skyzfs-coding-m1.2`) and tell the lead if your session
   cannot push it.

The lead's own changes to the design document and this guide go
straight to `zfs-topology`.

The design is also open for outside review as a documents-only pull
request into `main`, from the branch `skyzfs-design-docs`
(skyzfs/skyzfs#2). The lead copies `docs/skyzfs-design.md` and
`docs/skynbd-postgres-study.md` to that branch whenever they change and
answers its comments. Nobody else pushes there, and the user decides
when it merges.

## Easy to miss

- **No kernel features in cloud containers.** There is no ublk, ZFS,
  device-mapper or loop device here. Anything that needs one runs in
  GitHub Actions. Pure Rust builds and tests run locally.
- **The workspace needs two system packages to build**, because of
  `skynbd-agent`:
  `apt-get update && apt-get install -y --no-install-recommends libdevmapper-dev libudev-dev`.
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
- **The kernel probe.** `skyzfs-kernel-probe.yml` runs on a pull request
  that touches `tests/kernel/` or the workflow, and by dispatch. It
  fails when a property the design depends on does not hold on the
  runner (`pin_*` lines in its summary).
- **The replay rig.** `skyzfs-replay-rig.yml` runs on a pull request
  that touches `tests/kernel/`, and by dispatch. It has two kernel jobs,
  one per pool layout, of about twenty and twenty-six minutes. It
  records a pool's writes and checks the pool at every record prefix,
  every copy of every reachable block at the first and last prefix of
  each transaction group, and what `fsync` promised. It also fails when
  a negative control's result differs from the table `EXPECTED` in
  `tests/kernel/replay_check.py`, in either direction: changing what the
  rig catches means changing that table on purpose. A newer push to a
  pull request cancels the older run. The dispatch input `discard`
  chooses whether a discarded range is zeroed or left as it was.
- **The daemon rig.** `skyzfs-daemon-rig.yml` runs on a pull request
  that touches `tests/kernel/`, `bins/skyzfsd/`, `crates/skyzfs-ublk/`,
  `crates/skyzfs-blk/` or `bins/skyzfs-blkcheck/`, and by dispatch (input
  `checks`, for example `c` alone). It drives a server through the
  kernel and ZFS on Ubuntu 24.04 and 26.04, about sixteen minutes per
  image: parameters, `kill -9` under load, the conformance suite through
  the block device, a failed volume, a held request. Until `skyzfsd`
  serves, the server is a stand-in (`tests/kernel/skyzfsd-ref.sh`); the
  variables `RIG_SKYZFSD` and `RIG_SERVE_ARGS` point the rig at the
  daemon. The summary says for each line whether it rests on the kernel,
  on the server, or only shows that the check works. Exit 2 is an error
  of the rig and not a verdict on the server. Read the summary's table
  and not the colour. When check c fails, download the run's
  `c-trace.tsv.gz` and run `skyzfs-blkcheck --replay c-trace.tsv VOLUME`:
  it replays that volume's calls through the model with no kernel and
  prints the same violation, or shows that the rig's bookkeeping and
  not the server was at fault.
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
