# Contributing — TDD workflow

This repo is built test-first against `docs/skynbd_design.md`. The design
doc is the spec; the test suite is the spec made executable.

## The loop

1. Pick the next milestone item (design §15) — each maps to a crate and a
   design section.
2. **Write the failing test first**, at the lowest layer that can express
   it (design §14.1):
   - codec/protocol details → unit test with golden bytes (`skynbd-format`,
     `skynbd-nbd`)
   - semantic behavior → property test vs a reference model
     (`skynbd-tree::model_tests` is the pattern to copy)
   - durability/crash behavior → integration test against `MemoryStore`
     first, then the fault-injecting store + MinIO (M1+)
   - user-visible guarantees → an E2E scenario S1–S14 (design §14.4)
3. Implement until green. `cargo test --workspace` must pass at every
   commit on `main`.
4. Every invariant you touch (I1–I9, design §6.2) must name the test that
   verifies it in the test's doc comment.

## Ground rules

- No `unwrap()` outside tests; errors are typed per crate.
- `cargo fmt` + `cargo clippy -D warnings` are CI-enforced.
- Commit `proptest-regressions/` seed files — they are regression tests.
- Layering (design §11): no NBD knowledge outside `skynbd-nbd`, no S3
  knowledge outside `skynbd-store`, no IO in `skynbd-types`/`-tree`.
- Formats are versioned (design §5.2): changing serialized bytes requires
  a version bump and a cross-version decode test.

## Running tests

```sh
cargo test --workspace          # unit + property tests (no network)
cargo test -p skynbd-tree       # one crate
# M1+: RUN_MINIO=1 cargo test -p tests-integration  (docker required)
# M2+: tests/e2e/run.sh S1      (qemu-utils required)
```
