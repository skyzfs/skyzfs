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
own branch.

| Session | Role | Branch |
|---|---|---|
| skyzfs lead | Plans work packages, integrates, owns `docs/skyzfs-design.md` | `zfs-topology` (integration) |
| skyzfs coding | Implements work packages | `skyzfs-coding` |
| skyzfs testing | CI, test rigs and benchmarks, and running them | `skyzfs-testing` |
| skyzfs review | Reviews diffs. Does not change product code | `skyzfs-review` (notes only, if any) |

- Start each work package by merging the tip of `origin/zfs-topology`
  into your branch.
- The lead merges role branches into `zfs-topology` after review and
  green tests.
- No pull requests unless the user asks for one.
- The user may join any session and redirect it. The user's instructions
  take precedence over a work package. Tell the lead what changed.

## Easy to miss

- **No kernel features in cloud containers.** There is no ublk, ZFS,
  device-mapper or loop device here. Anything that needs one runs in
  GitHub Actions. Pure Rust builds and tests run locally.
- **CI coverage.** `ci.yml`, `coverage.yml` and `e2e.yml` run on `main`
  and on pull requests only. A push to a role branch is not covered by
  them.
- **The benchmark's push trigger.** A push to any branch but `main` that
  touches `topology-bench.sh`, `tests/lib/gnu-dd.sh` or the workflow
  starts thirteen runner jobs. Cancel the run if you did not want it.
- **Runner noise.** Two GitHub runner VMs differ by more than most
  effects being measured. Compare configurations only on one runner (the
  workflow's `sequence` input), and say how many runs a number rests on.
- **Do not weaken a test to make it pass.** Report it.
- **Commit messages** say what changed and why, in plain words. No model
  names and no session identifiers.

## Reporting a work package

Send the lead a message with:

1. the branch and the commit range,
2. what was done against each acceptance criterion,
3. how it was verified: the commands and their results, and links to any
   CI runs,
4. what is not done, and what you are unsure of.
