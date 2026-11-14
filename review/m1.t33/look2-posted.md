## Second look at M1.T33: 7aaae9c, 0486339 and f3f7d77

**One finding blocks the merge: the flush course fails on all three images.** The fault is in the control program (`skyzfs-ublk-control`), not in the rig or the kernel. The rig is right to fail it, and f3f7d77 now says why.

The rest of what the lead asked for reads as it should in run 43569917318 at f3f7d77:
- **The write:** the reissued write lands. It reads back through the device and from the image, and the writer gets success.
- **The delete branch:** each kind's client gets EIO: `dd: error reading`, `dd: error writing` and `dd: fsync failed`. The delete returns within 30 ms on every image. For the write, the image keeps the old block.
- **The pins:** every other `refused_*` pin is PASS.

I found no defect in the rig's own code. `test_control_refused.sh` passes at f3f7d77. I planted twelve changes (table below).

### Blocking: the program panics on the flush it is asked to answer

**What the run shows.** `pin_refused_flush_completes_after_take_over` FAILs on ubuntu-24.04, 26.04 and 26.04-arm. It is the only line that differs from the table. The take-over ended by itself with exit 1. The 24.04 artifact's `control.err` has:

```
thread 'skyzfsd-queue' (16288) panicked at crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs:370:18:
attempt to multiply with overflow
```

**The cause (certain).**
- Line 370 is the first line of `answer()`: `let at = f.desc.start_sector * 512;`.
- It runs before the match on the request's kind, so it also runs for a flush.
- A flush has no sector, and its start sector overflows the product. The rig builds the program in debug, where an overflow panics.
- `covers()` and `named()` already use `saturating_mul`. That is why the first server could name and refuse the flush without trouble.

**A reproducer with no kernel.** I added this test to the program's own tests, which use its `Scripted` queue. It panics at the same line and column, `370:18`. With `f.desc.start_sector.saturating_mul(512)` on line 370 it passes.

```rust
#[test]
fn a_flush_with_no_sector_is_answered() {
    let flush = Fetched {
        tag: 2,
        desc: Desc { op_flags: u32::from(op::FLUSH), nr_sectors: 0, start_sector: u64::MAX },
    };
    let (queue, _lines, counts) =
        run(vec![(vec![flush], Vec::new(), Ok(Woken::Work))], Plan::default());
    assert_eq!(queue.commits, vec![(2, 0, false)]);
    assert_eq!(counts.flushes.load(Ordering::Relaxed), 1);
}
```

**How far it reaches (fairly sure, not checked against the source).**
- I believe the block layer gives the kernel's flush request the sector `(sector_t)-1` (`blk_rq_init`). If so, any flush the program answers panics it, not only a reissued one. I could not fetch the kernel's source from here to check.
- The run cannot settle it either: no server in it answered a flush. Every served line reads `flushes 0`, the write take-over's included.
- So this is likely the first time the program has been asked to answer a flush on a kernel.

**The daemon is not affected.** `translate()` returns the flush's plan before it reads the start sector (`crates/skyzfs-ublk/src/translate.rs`). Its other uses saturate (`delay.rs`) or only print the value (`record.rs`).

**Where the fix goes.** The program is outside this PR's diff; it came with #71. The fix is that one line, plus the test above, which holds it with no kernel. The lead decides who makes it and whether it goes in this PR or before it. After it, the flush course wants a new kernel run.

### f3f7d77: a take-over that ended by itself is named

- **Sound.** The check uses `kill -0` on the take-over, after the client's wait. If the take-over has gone, the pin is FAIL whatever the client did, and the line names the exit and the error.
- **The test holds both parts.** In `take_over_dies` the made-up client still completes. So the course fails on the new check alone (T2 below), and not just because the client waited.
- **Suggestion.** The line quotes only the last line of `control.err`, which is the program's own "the queue's thread panicked". The two lines that say where and why are three lines up. The line could quote the `panicked at` line and the one after it when there is one. That is what took me to line 370.

### `refused_course`'s write branch

You asked me to look hardest at this. It is right.
- **The program serves nothing it refuses.** `--refuse-one-commit write:B` commits the request at once with no buffer (`server` in `skyzfs-ublk-control.rs`), without writing it to the file. So the image holds the old block until a server answers the reissued write. A new block found there after the take-over can only have come from the reissue. In the run, the delete course's image still has the old block, as it should.
- **The writer.** It writes 4 KiB of new data at B with `O_DIRECT`, so no page cache of the device stands between the writer and the server.
- **The pin needs all three:**
  - the writer ends with 0 within 60 s of the take-over;
  - the block at B, read through the device (`O_DIRECT`) while the take-over serves, is the new data;
  - the image's block, read after the take-over has ended, is the new data too.

  Each is planted alone below, and each plant fails a course: `lost_write`, `stale_device`, `image_lost` or `writer_fails`.
- **Both blocks are random.** The old block and the new data are random. So a server that answered from zeros, or wrote the wrong range, cannot pass by chance.

### The flush course

- **The flush reaches the driver.** The binding's devices have `UBLK_ATTR_VOLATILE_CACHE` (`queue.rs`). So the client's `fsync`, which writes nothing (`dd if=/dev/null ... conv=fsync`), is sent on as a flush. The run shows it: the first server named and refused a flush.
- **Suggestion: pin which flush was refused.** The step matches a flush by kind alone: `What::Flush`, and "sent for a flush on tag T". If something else sent a flush before the client, that flush would be refused instead. The client's own flush would then wait behind the quiesced device and complete after the take-over. Both pins would PASS, and the refused flush would never be shown to complete.

  The run makes that unlikely: the first server had answered no flush (`flushes 0`) when it refused. But nothing in the step rules it out.

  The take-over's `served` line tells the two cases apart. It counts one flush when the refused one was the client's: the reissue. It counts two when another was refused: the reissue and the client's. Pinning `flushes 1` in that line would hold it, once the program can answer a flush. The same check for the write course (`writes 1`) costs nothing; the data check already holds that one.
- **What the made-up `dd` cannot show.** It reads any client with `if=/dev/null` as a flush. So taking `fsync` out of the client's `conv` (W5) passes every test. Only a kernel shows that the client's flush is sent, and on a kernel the step fails without it.

### Both suggestions of my first look are taken

- **R2 and R4** are now each held: `aborted_then_ended` fails without R2's check, and `refused_crashes` without R4's (table).
- **`/run/ublksrvd`** is now looked at for 44, 45 and 50 to 54.

### Planted changes

One at a time. After each I ran `test_control_check.py` and `test_control_refused.sh` (T1 and T2: the latter only).

| plant | result |
|---|---|
| R1b: the sent line is not checked (any request passes) | 3 courses fail: a read refused where a write or a flush was to be, and another read |
| R2: the driver's answer is not checked | `aborted_then_ended` fails (at 0264071: nothing) |
| R4: the program's last line is not checked | `refused_crashes` fails (at 0264071: nothing) |
| W1: the write is not read back through the device | `stale_device` fails |
| W2: the write is not read back from the image | `image_lost` fails |
| W3: the write is not read back at all | `lost_write`, `stale_device` and `image_lost` fail |
| W4: a writer that ended with an error passes | `writer_fails` fails |
| **W5: the flush client does not `fsync`** | **nothing fails**: only a kernel shows it (above) |
| W6: the write course refuses a read | "write: the server ends by the refusal" fails |
| W7: the flush course's take-over pin is the read pin | `test_every_line_the_rig_names_is_in_the_table` and two checks fail |
| T1: the take-over's own end is not looked at | "a take-over that ended by itself fails" and both naming checks fail |
| T2: a take-over that ended by itself is named, not failed | "a take-over that ended by itself fails" fails |

### Remarks (not blocking)

1. **A take-over pin after a failed step.** `refused_course` judges the take-over pin whenever the step is not ERROR. Suppose the step FAILed because the client had already ended. Then `rc_client` reads that earlier end.
   - A client that ended with an error makes the take-over pin FAIL too, which is fine.
   - A client that ended with success makes it PASS, though the take-over did nothing.

   The step's own FAIL keeps the run from reading green, so this changes no verdict. But the take-over's line would claim what it did not see. One way out: judge the take-over pin only when the client was still waiting at the step's end. Otherwise FAIL it with "the client had ended before the take-over".
2. **`obs_refused_read_then_delete`** says the client's data "is NEITHER the old block nor the written data". The client got EIO and read nothing, so "no data" would say it without sounding like a finding.

### CI at f3f7d77, when I wrote this

- **Failed:** the control rig, on the flush pin only (above).
- **Green:** ci.yml's test, coverage, e2e and the kernel probe. Also the replay rig's daemon job on 24.04 and both "write into a hole" jobs.
- **Still running:** the daemon rig (daemon and stand-in), the replay rig's other jobs, and ci.yml's lint and unit-arm64.

---
_Generated by [Claude Code](https://claude.ai/code)_
