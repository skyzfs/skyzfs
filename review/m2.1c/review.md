## Review of #96 (M2.1c: an uberblock counts only where its label checksum holds), commit 58bb9dc at head 8da0a73

**Verdict: nothing here blocks the merge.** The checksum is ZFS's: I checked it against the sources, against an implementation of my own over every recorded uberblock, and against known answers in both byte orders. I found no defect in what ZFS can write through this device.

Two things remain:

- **S1 needs a decision on scope.** The view still follows a slot by its first 200 bytes, and since this change an uberblock is valid by its whole slot. Codex's P2 is one direction of it; there is a second. I found no way for ZFS to reach either, so I do not hold the merge for it. I would close it before M2.5, since a publish will act on `cut_opens`.
- **S2 is six test gaps.** The first is your question 3: no test fails when the rings read at the start go unchecked. Six tests for them are in `review/m2.1c/reproducers.patch` on `skyzfs-review`. They pass at 8da0a73 and each fails under its plant.

CI at 8da0a73: everything has passed except the daemon rig's three jobs, which were still running when I wrote this.

### How I checked

- **The sources.** I read 58bb9dc against the M2.1c row and OpenZFS 2.4.1: `zio_checksum.c`, `sha2_zfs.c`, `zio.h`, `vdev_impl.h`, `vdev_label.c`.
- **An implementation of my own.** `review/m2.1c/label_cksum.py` is written from the sources alone, with Python's hashlib and no code of this pull request. Over the nine recordings it finds 1,482 uberblocks:
  - every one verifies at 4 KiB;
  - none verifies at another size;
  - none is of group 0.
- **Known answers.** `review/m2.1c/vector.py` computes one slot's checksum as a little-endian and as a big-endian host writes it. `seal` gives the same 32 bytes in both orders.
- **The tests** pass here (rustc 1.100.0): `skyzfs-uberblock` 15, `skyzfs-local` 163.
- **Planted changes.** I planted 26, one at a time (`review/m2.1c/plants.py`, results in `plants.out`). The tests caught 17. Of the nine survivors:
  - three change nothing: trying the sizes largest first, aligning on the volume and not in the ring, and trying a slot the write holds in part;
  - six are S2.

### Findings (none blocks)

**S1. The view follows a slot by its first 200 bytes, but an uberblock is now valid by its whole slot.** (`skyzfs-uberblock/src/lib.rs:552`, `View::changed`, the test at line 558. Confidence: high that it happens, both directions run here; low that anything but a foreign writer reaches it.)

Two directions, shown on an 8 KiB slot (`review/m2.1c/s1_test.rs`, which fails at 8da0a73):

| Sequence | On the volume | In the view |
|---|---|---|
| A slot is written whole, then its second half is zeroed or overwritten (Codex's case) | The checksum fails: an import passes over it | Still held, and `chosen` still says yes |
| The same slot arrives as two writes, head then tail | The whole valid slot | Its place is known and empty |

- **Why it matters.** Either can make a cut say `itself` wrongly:
  - the first when the stale copy is the only one of the cut's state;
  - the second when the uberblock the view missed is greater than the cut's.
- **What can reach it.** I found no way for ZFS to, through this device:
  - ZFS writes a slot as one I/O of at most 8 KiB;
  - the device has 4 KiB blocks, requests up to 1 MiB and no chunk or boundary limit (`skyzfs-ublk-sys/src/queue.rs:428`), so the block layer has no reason to split such a write;
  - ZFS never trims or zeroes a label.

  What does write into a ring otherwise is outside the kernel's ZFS. Examples are `zpool labelclear`, which writes 240 KiB of zeros per label through the page cache (`libzfs_import.c:136`), and `dd`.
- **How it differs from the description's open question 1.** There the view is right about what an import would read. Here it is not.
- **A fix that costs ZFS's own writes nothing:**
  - a held uberblock covers its whole verified slot (`Uberblock::slot` has the size): a change that overlaps any of it without holding the slot whole drops it and leaves its place unknown;
  - an uberblock found where a size that could begin there is not held whole, and no held size verifies, leaves its place unknown and not empty.

  With both, either sequence above ends unknown, so no cut says `itself` until the slot is written whole again. The forged write of `bytes_zfs_would_pass_over_make_no_cut` is unaffected: its place begins no 8 KiB slot, so every size is held whole.
- **The other way to settle it** is to say in the module's readings that the view is right only while every write into a ring is a whole slot, and what it says when one is not.

**S2. Six plants pass every test.** Each has a test in the patch.

1. **The rings read at the start are not checked** (`cuts.rs:313` using `uberblocks_in` unfiltered).
   - The mutant turns off both paths at once, and its test never starts a second store.
   - Scenario: a first attachment leaves unsealed bytes of group 99 in a ring. The next start holds them as the greatest in view, and every cut says `rewind` until that slot is written again.
   - Test: `bytes_zfs_would_pass_over_at_the_start_are_not_in_view`.
2. **A big-endian writer's verifier is written little endian** (`lib.rs:383`, `label_checksum` ignoring `order` for the offset).
   - `seal` and `label_checksum_holds` share `label_checksum`, so a slot the tests seal themselves cannot show an error both make. No recording is from a big-endian host.
   - Test: `the_label_checksum_gives_the_answers_zfs_gives`, with the answers from `vector.py`.
3. **Only the first of the four words is compared** (`lib.rs:416`). The same test changes a byte of each word and of the checksum's magic.
4. **A slot need not be aligned in the ring** (`lib.rs:274`). Test: `a_slot_out_of_step_with_the_ring_is_not_verified`.
5. **A slot whose checksum has no magic is refused** (`lib.rs:402`).
   - The description's open question 2 states the reading, and it is ZFS's: for a label, `zio_checksum_error_impl` asks only whether the magic reads swapped (`zio_checksum.c:481`).
   - Test: `a_checksum_without_its_magic_is_read_as_a_little_endian_host_reads_it`.
6. **`View::saw` keeps the unverified** (`lib.rs:537`).
   - `saw` has no caller left in the product: the store goes through `CutRule::found` and `View::changed`. So the view's own filter runs only in tests, and only on sealed slots.
   - Test: `the_view_holds_no_uberblock_whose_checksum_fails`. Or let one place hold the filter.

**S3. Smaller notes.**

- **What the recordings cover.** All nine are ZFS 2.4.1 on x86_64 with 4 KiB slots.
  - `every_recorded_uberblock_is_verified_by_its_label_checksum` says "at the slot's own size" and asserts only that some size holds.
  - Slots of 1, 2 and 8 KiB and the big-endian order rest on the crate's own `seal`, which is why S2's second test is worth having.
- **Slots of 1 and 2 KiB cannot occur on this device.** Its logical block is 4 KiB, so a pool's `ashift` is at least 12. At `ashift` 12 each uberblock write hashes 3 KiB that cannot hold. The cost is next to nothing, as your table shows, and trying the write's own length first, the refinement you left out, would save it.
- **`the_checksum_is_read_in_the_order_its_magic_shows`.** Its second assert fails for a reason other than the one its doc gives: the magic is hashed, so changing it breaks the checksum whatever order the words are read in. The plants on the order are caught all the same, by the first assert and by `an_uberblock_is_verified_by_its_label_checksum`.

### Your three questions

**1. The checksum's steps: each matches the source.**

| Step | Source | Here |
|---|---|---|
| The verifier is `(offset, 0, 0, 0)` | `zio_checksum.c:297` | `lib.rs:383` |
| The offset is the I/O's on the leaf vdev, which is the volume's since the vdev is the whole volume | `vdev_label.c:189` to `216`, `zio->io_offset` | `at` |
| The magic and the verifier are in place when the whole slot is hashed | `zio_checksum_compute`, `zio_checksum.c:347` | the hash covers all but the last 32 bytes, then the verifier |
| Each word is the digest's next eight bytes read big endian, on either host | `sha2_zfs.c:74` | `ByteOrder::Big.word` |
| The words and the magic are stored in the writer's order | `zio_eck_t`, `zio.h:53` | `order.bytes` |
| A reader swaps the verifier and the stored words when the magic reads swapped, and hashes the same way | `zio_checksum.c:481` to `491`; both functions of the label's entry are `abd_checksum_sha256` (line 175) | `label_checksum_holds` |

- The line numbers in the module's docs are right.
- A big-endian host's slot ends in the digest's 32 bytes as they are. A little-endian host's ends in each eight of them reversed. The known answers show both.

**2. Trying 1, 2, 4 and 8 KiB, smallest first: right, and the order does not change the result.**

- **The sizes** are ZFS's: the `ashift` held between 10 and 13 (`vdev_impl.h:491`).
- **The alignment** is ZFS's: a slot begins at a multiple of its size from the ring's start (`vdev_impl.h:497`).
- **Two sizes cannot both hold** short of a SHA-256 that ends where the smaller slot's zeros are. Among the 1,482 recorded uberblocks none does, and the plant that tries largest first passes every test.
- **The limit:** a sealed slot of a size other than the pool's is taken, though that pool's import would not read it. Only a foreign writer makes one.

**3. The check applies on both paths in the code; only the write path has a test.**

- `received` (`cuts.rs:336`) and `read_at_start` (`cuts.rs:313`) both go through `found`.
- At the start a ring is read as one buffer, so every slot is held whole.
- This also closes what I named on #95: a slot torn at a crash is now passed over at the start, as an import passes over it.
- The plant on the write path is caught by `bytes_zfs_would_pass_over_make_no_cut`. The plant on the start path is S2's first.

### On Codex's finding

I agree with the observation and ran it: the view goes on choosing the uberblock after its slot's second half is zeroed. S1 has it with its reverse, what I found on reaching it, and a fix. I would not rate it above a suggestion while only a foreign writer reaches it.
