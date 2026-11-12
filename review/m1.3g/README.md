# M1.3g (pull request 23), first look at 06b77f9

Base 44fb9e9. Tests of skyzfs-blk, -local, -ublk, -ublk-sys, skyzfsd
(also with `test-hooks`): 556 passed, 0 failed, 12 ignored; clippy and
fmt clean. No kernel.

## A subdirectory in the volume's directory

Temporary test in `crates/skyzfs-local/tests/store_real.rs` (real
files, ext4 here): create a store, drop it, then

| the directory also holds | `Store::open` at 06b77f9 | at the base |
|---|---|---|
| nothing else | ok | ok |
| a subdirectory `lost+found` | refused: "the block size of the file system under a file in the volume's directory could not be read (IsADirectory): whether a write there stays within the volume's write unit cannot be checked" | ok (with a stray file beside it too) |
| a dangling symbolic link | ok (not found, skipped) | not tried |

`blocks_within_the_unit` opens every listed name read-write and treats
any error but "not found" as an unreadable block size.

## Which stage the exposure test fails at

`a_volume_whose_unit_is_below_the_block_loses_what_nobody_wrote`, with
the outcome printed: all four environments fail at the first stage,
"flushed, after a cut behind a later write: block 0 holds 0x00 at byte
0, not 0x09". That stage has writes only, no hole. The checkpoint stage
is never reached with the checks off.
