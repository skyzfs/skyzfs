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

## Easy to miss

- **No kernel features in cloud containers.** There is no ublk, ZFS,
  device-mapper or loop device here. Anything that needs one runs in
  GitHub Actions. Pure Rust builds and tests run locally.
- **The workspace needs two system packages to build**, because of
  `skynbd-agent`:
  `apt-get update && apt-get install -y --no-install-recommends libdevmapper-dev libudev-dev`.
- **CI coverage.** `ci.yml`, `coverage.yml` and `e2e.yml` run on `main`
  and on every pull request, whatever its base. A push to a role branch
  with no pull request open is not covered by them.
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

The pull request's description is the report. It has:

1. the work package and the commit range,
2. what was done against each acceptance criterion,
3. how it was verified: the commands and their results, and links to any
   CI runs,
4. what is not done, and what you are unsure of.

Send the lead a message with the link and anything that needs a
decision. A reviewer who is not on this project reads the description
cold, so write it for them.
