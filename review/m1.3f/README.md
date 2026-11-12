# M1.3f (pull request 21), first look at 5486d01

Base 8899c01. Tests of skyzfs-blk, -local, -ublk, -ublk-sys, skyzfsd
(also with `test-hooks`): 543 passed, 0 failed, 12 ignored; clippy both
ways and fmt clean. The real-file tests ran on ext4 here and
deallocated. No kernel.

## One temporary test (not in the branch)

The new cut-anywhere test zeroes blocks 1 to 5 of each group and writes
block 7: no write lands inside a range zeroed earlier in the log. A
temporary test in `store/tests.rs`, same harness, all environments:

- each of 128 groups: write-zeroes over blocks 1..5, then a write of
  block 3; in every third group also write-zeroes over blocks 2..3 and
  then a write of block 2;
- steps of at most 5 records, so a step often ends between a
  write-zeroes and the later write into its range;
- halted after 0..39, 44, 61, 90, 150, 300, 700 operations, power cut,
  an open halted and cut again, then a full open;
- checked after the open, after the remaining steps, and after one
  more open: each block reads its newest write, or zeros.

It passed. Outline of the body: as `steps_of_holes_and_writes_can_be_
cut_anywhere`, with the records and expectations above and
`holds(scenario)`.
