# SkyNBD

[![ci](https://github.com/skyzfs/skyzfs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/skyzfs/skyzfs/actions/workflows/ci.yml)
[![e2e](https://github.com/skyzfs/skyzfs/actions/workflows/e2e.yml/badge.svg?branch=main)](https://github.com/skyzfs/skyzfs/actions/workflows/e2e.yml)
[![nightly](https://github.com/skyzfs/skyzfs/actions/workflows/nightly.yml/badge.svg)](https://github.com/skyzfs/skyzfs/actions/workflows/nightly.yml)
[![tests](https://github.com/skyzfs/skyzfs/raw/badges/main/tests.svg)](https://github.com/skyzfs/skyzfs/actions/workflows/coverage.yml?query=branch%3Amain)
[![coverage](https://github.com/skyzfs/skyzfs/raw/badges/main/coverage.svg)](https://github.com/skyzfs/skyzfs/actions/workflows/coverage.yml?query=branch%3Amain)

<!-- ci = fmt/clippy/unit+property+S15 · e2e = Tier A scenarios (qemu-io,
     MinIO) · nightly = Tier B kernel rig (S4/S14/S14a/S10) · tests =
     tests passed in main's coverage run · coverage = library line
     coverage, gated at ≥85% (baseline 90.0%). The tests and coverage
     SVGs are rendered by coverage.yml's publish-badges job onto the
     `badges` branch (scripts/ci/publish-badges.sh); served via
     github.com/…/raw/ so they work in this private repo. -->

Snapshot-efficient NBD-as-a-service backed by S3-compatible object
storage, written in Rust.

A SkyNBD server exposes Linux NBD block devices whose data and metadata
live entirely in S3 (MinIO / AWS S3 / Ceph RGW). Snapshots are the core
feature, not an afterthought: O(1) creation, instantly mountable read-only
exports, content-addressed sharing between snapshots, diff cost
proportional to changed data, and a control plane for externally
coordinated filesystem-consistent snapshots — driven in production by the
bundled LVM-free `skynbd-agent` (raw dm-thin, one static binary).

## Quick start

Build the daemon and CLI (the `s3` feature enables the S3 store backend
alongside the in-memory one):

```sh
cargo build --release -p skynbdd -p skynbdctl --features skynbdd/s3,skynbdctl/s3
```

Point it at any S3-compatible endpoint — or none. The smallest useful
config, `skynbd.toml`:

```toml
nbd_listen = "127.0.0.1:10809"   # or "unix:/run/skynbd/nbd.sock"
api_listen = "127.0.0.1:8080"
refs_path  = "refs.redb"         # omit → in-memory refs (no restarts)

[store]
kind = "memory"                  # RAM-only; see below for S3

[[volumes]]
name = "vol1"
size = 67108864                  # 64 MiB
```

For durable storage swap the `[store]` block for an S3 endpoint
(`kind = "s3"` plus `endpoint`/`bucket`/`access_key`/`secret_key`/
`region` — MinIO, moto, AWS all work; the bucket is created on demand).
Omit the keys entirely and credentials come from the ambient AWS chain
instead — web identity (`AWS_WEB_IDENTITY_TOKEN_FILE`), env keys,
profiles, IMDS — so production configs carry no secrets
(`docs/skynbd-auth-design.md` §5); set `create_bucket = false` there too,
since least-privilege roles won't have `s3:CreateBucket`.

Run it, then talk to it with stock tools — no root needed:

```sh
./target/release/skynbdd skynbd.toml
```

```sh
# Block device I/O over NBD (qemu-utils + libnbd-bin):
URI=nbd://127.0.0.1:10809/vol1/head
nbdinfo --list nbd://127.0.0.1:10809          # exports, sizes, flags
qemu-io -f raw "$URI" -c 'write -P 0xab 0 1M' -c 'read -P 0xab 0 1M'

# Snapshot via the REST API — O(1), instantly exported read-only:
curl -s -X POST http://127.0.0.1:8080/v1/volumes/vol1/snapshots \
  -H 'content-type: application/json' -d '{"name":"s1"}'
qemu-img convert -f raw -O raw "nbd://127.0.0.1:10809/vol1/snap/s1" s1.img
```

To attach a device with the kernel client instead:
`sudo nbd-client -N vol1/head 127.0.0.1 10809 /dev/nbd0` (root, and a
kernel with the `nbd` module). With an S3 store,
`skynbdctl fsck skynbd.toml vol1` checks every invariant offline (and
`--rebuild-refs` repairs the ref database from the object store).

For filesystem-consistent snapshots of a *client* host, run
[`skynbd-agent`](docs/skynbd-agent-design.md) on that host against a
lineage volume: it freezes the filesystem, snapshots a raw dm-thin pool,
and ships only the delta — no lvm2 or thin-provisioning-tools installed.

The full tour — S3 store setup, the REST cookbook, ingestion-plane
recipes, agent debugging, coverage runs, and the root/Tier B rigs — is in
**[`docs/manual-testing.md`](docs/manual-testing.md)**.

## Documentation

| Doc | What it is |
|---|---|
| [`docs/skynbd_design.md`](docs/skynbd_design.md) | **The spec.** Server design v3: formats, commit protocol, NBD subset, snapshot subsystem, V3.2 ingestion control plane (§9A), E2E scenarios S1–S14, milestones. |
| [`docs/skynbd-agent-design.md`](docs/skynbd-agent-design.md) | Production snapshot agent (client host): durable state machine, recovery planner, freeze watchdog, `DmOps` kernel seam, pool safety — implements V3.2. |
| [`docs/skynbd-snapshot-design-v3_2.md`](docs/skynbd-snapshot-design-v3_2.md) | Client-side filesystem-consistent snapshot design (V3.2): fsfreeze + dm-thin baseline pairs + deltas; defines the control-plane contract the server implements. |
| [`docs/manual-testing.md`](docs/manual-testing.md) | Manual testing & debugging cookbook: every subsystem poked with standard CLI tools. |
| [`tests/README.md`](tests/README.md) | Test layout, the reference coordinator, and the nightly campaign's lessons table. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | TDD workflow — how code gets added here. |

## Developing

```sh
cargo test --workspace   # unit/property/integration incl. crash matrices; no network needed
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings

# Tier A e2e scenarios (qemu-utils + any S3 endpoint, e.g. moto/MinIO):
SKYNBD_S3_TEST_ENDPOINT=http://127.0.0.1:9010 tests/e2e/run.sh S1
```

Tier A e2e scenarios run on every PR; Tier B scenarios need root, kernel
NBD and a dm-thin stack, and run in the scheduled
[`nightly.yml`](.github/workflows/nightly.yml) workflow (locally:
`sudo -E tests/e2e/run.sh S4 S14 S14a S10`).

Start with `CONTRIBUTING.md`; the short version is *no code without a
failing test first*, and the design doc section number goes in the test's
doc comment.

## License

Copyright 2026 Sky Computing LLC. Licensed under the Functional Source
License, Version 1.1, ALv2 Future License (`FSL-1.1-ALv2`). See
[`LICENSE`](LICENSE) for the full text. Each version becomes available
under the Apache License, Version 2.0 on the second anniversary of its
release.
