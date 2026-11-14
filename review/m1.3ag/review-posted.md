## Review of M1.3ag at a0318b6

**Nothing blocks, and I found no defect.** It can merge once CI, the daemon rig and the replay rig's daemon jobs are green on a0318b6. The fix is complete, the daemon's flush path is as the description says, and each new test fails when what it claims is broken.

**One thing for the description and the comments.** A run on a kernel already shows what a flush's sector holds: u64::MAX, as the reading of `blk_rq_init` says (below). So "not checked on a kernel" can give way to that run.

I read the diff before the description. At a0318b6 with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings` on `skyzfs-ublk` and `skyzfs-ublk-sys`, plain and with `control-test-program`;
- the tests of `skyzfs-ublk` (138 in the lib), of the program (13) and of `skyzfs-ublk-sys`.

I planted seven changes (table below).

### What you asked me to judge

**1. Is the fix complete? Yes.**
- **The place is computed only where it is used.** `at` is now a closure, called only by a read, a write, a discard and a write of zeros. A flush never computes it. `bytes` cannot overflow: a `u32` of sectors times 512 fits easily.
- **Nothing else in the program computes from the sector unchecked.** `covers()` and `named()` saturate, and the rest only copies the field. `KernelQueue` in `queue.rs` only copies it too.
- **`checked_mul` with `EIO` is the better choice than my `saturating_mul` on #80.**
  - It answers as the daemon answers any refusal: `Translator::refused` gives `-EIO`.
  - With `saturating_mul`, a place beyond 64 bits would reach `pread` at offset u64::MAX, and the answer would be whatever that returns.
  - Plant C2 shows the difference: `a_place_beyond_64_bits_is_answered_with_an_error` fails with it.
- **The three tests are test-first.** C0 puts back the old product, computed before the match. All three tests then panic with "attempt to multiply with overflow", as the rig did.

**2. Does the daemon's flush path compute a range from the sector? No.**
- I listed every use of `start_sector` and `nr_sectors` outside tests:
  - in `crates/skyzfs-ublk/src/`, `crates/skyzfs-ublk-sys/src/` and `bins/skyzfsd/src/`: they are the ones the description names;
  - in `queue.rs`: a copy of the field;
  - in `bins/skyzfs-blkcheck/src`: none.
- `pump.rs:752` is the only call to `translate()`. `translate()` returns `Action::Flush` at lines 291–296, before `range()`. `refused()` reads only the operation.
- **The new pump test holds it, and it adds something.**
  - D1 checks a flush's range as any other request's. It fails the new test and five others.
  - D2 refuses a flush whose sector is not inside the device. The new test fails, and so does `the_five_operations_are_translated_under_every_hint_flag`, but not the old pump test of a flush, which uses sector 0.
  - D3 computes a flush's byte unchecked. It fails the new test.

  So the sector u64::MAX is held through the pump as well as in the translator's own test.
- **A small error in the new test's comment.** It says a translation that computed a range "would refuse both (out of the device, or empty)". `range()` refuses the first as empty. It refuses the second as not on 4 KiB blocks (u64::MAX % 8 = 7) before it looks at the device's end. "(empty, or not on 4 KiB blocks)" would be exact.

**3. Is the kernel reading marked as unchecked wherever it is used? Yes, nearly. And it can now be marked as measured.**
- **Marked:** `answer()`'s comment ("not checked on a kernel"), `a_flush_is_answered_whatever_its_sector_reads`, the pump test ("by a reading of the kernel's source"), the description and the first commit's message.
- **Not marked:** `a_refused_flush_handed_over_again_is_answered` says "with no sector" as a fact.
- **The measurement.** The daemon's request log (`--request-log`) writes the sector of every request it is handed. The direct-write split probe (`skyzfs-dio-split.yml`) runs check c through the daemon's M1.3y build with that log, and keeps the logs. Its run 43455086383 (artifact `dio-split-ubuntu-26.04`, Ubuntu 26.04.1, kernel 7.0.0-1019-azure) has them for two passes.
  - Pass 1: 17,205 requests, of which 157 were flushes.
  - Pass 3: 17,935 requests, of which 173 were flushes.
  - Every one of the 330 flushes reads sector `18446744073709551615` with 0 sectors.
  - Every flush answer in the logs is 0 (321 answers). The daemon has been answering such flushes all along.
- **What it does not cover.** It is one kernel, on amd64. On 24.04 and on arm64, the control rig's panic shows only that the field is at least 2^55.

  So "a reading of the kernel's source, not checked on a kernel" could become: "u64::MAX, as `blk_rq_init` sets it: the daemon's request log shows it on every flush of dio-split run 43455086383 (26.04, kernel 7.0)". The "Not done" item about a measurement could then go.

### Planted changes

One at a time, each with the tests of the crate it is in. `skyzfs-ublk`'s lib is for D1 to D3; the program's tests are for C0 to C3.

| plant | result |
|---|---|
| C0: the old product, computed before the match | all three new program tests fail, each panicking at the multiplication |
| C1: the product unchecked, but only where a place is used | `a_place_beyond_64_bits_is_answered_with_an_error` fails |
| C2: `saturating_mul` in place of `checked_mul` | `a_place_beyond_64_bits_is_answered_with_an_error` fails |
| C3: every request's place checked first, a flush's too | all three new program tests fail |
| D1: a flush's range checked as any request's | the new pump test, the old pump flush test and four others fail |
| D2: a flush must start inside the device | the new pump test and `the_five_operations_are_translated_under_every_hint_flag` fail |
| D3: a flush's byte computed unchecked | the new pump test and two translator tests fail |

### CI on a0318b6, when I wrote this

- **Green:** the control rig on all three images (43570848360). It runs the courses on `zfs-topology`, so it does not run #80's flush course; that is #80's rerun after this merges, as you say. Also green: the replay rig's "write into a hole" jobs and the no-kernel jobs.
- **Still running:** the daemon rig on all three images, the replay rig's daemon jobs, ci.yml, coverage and e2e.

---
_Generated by [Claude Code](https://claude.ai/code)_
