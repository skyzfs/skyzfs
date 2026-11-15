//! The parser held against the on-disk format as the crate's
//! documentation cites it. Pools recorded on real ZFS (M2.T1) are held
//! against it in `tests/recorded.rs` once they are there.

use super::*;

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;

/// A slot of `size` bytes as `vdev_uberblock_sync` writes it, in
/// `order`: the uberblock at its start (group `txg`, a root block
/// pointer made from `root`, and the fields after it), zeros, and the
/// label's checksum in its last 40 bytes, here five copies of the
/// magic, so that a reader that looked anywhere but at a whole KiB would
/// find one there.
fn slot(order: ByteOrder, txg: u64, root: u64, size: u64) -> Vec<u8> {
    let mut s = vec![0; size as usize];
    let put = |s: &mut [u8], at: usize, word: u64| {
        let bytes = match order {
            ByteOrder::Little => word.to_le_bytes(),
            ByteOrder::Big => word.to_be_bytes(),
        };
        s[at..at + 8].copy_from_slice(&bytes);
    };
    put(&mut s, 0, MAGIC);
    put(&mut s, 8, 5000);
    put(&mut s, TXG_AT, txg);
    put(&mut s, 24, 0x1234_5678_9abc_def0);
    put(&mut s, 32, 1_763_000_000 + txg);
    for (n, word) in root_of(root).into_iter().enumerate() {
        put(&mut s, ROOT_AT + 8 * n, word);
    }
    put(&mut s, ROOT_AT + ROOT_WORDS * 8, 5000);
    put(&mut s, MMP_MAGIC_AT, MMP_MAGIC);
    for k in 0..5 {
        put(&mut s, size as usize - 40 + 8 * k, MAGIC);
    }
    s
}

/// The root block pointer [`slot`] makes from `root`.
fn root_of(root: u64) -> [u64; ROOT_WORDS] {
    std::array::from_fn(|n| {
        root.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(n as u32)
            ^ n as u64
    })
}

fn state(txg: u64, root: u64) -> State {
    State {
        txg,
        root: root_of(root),
    }
}

/// Labels 0 and 1 at the start of the vdev and 2 and 3 at its end, the
/// vdev's size rounded down to a whole label (`vdev_label_offset`), and
/// each ring 128 KiB into its label.
#[test]
fn labels_lie_where_vdev_label_offset_puts_them() {
    let labels = Labels::for_size(64 * MIB);
    assert_eq!(labels.psize(), 64 * MIB);
    let at: Vec<_> = (0..LABELS).map(|l| labels.label_offset(l)).collect();
    assert_eq!(
        at,
        [
            Some(0),
            Some(256 * KIB),
            Some(64 * MIB - 512 * KIB),
            Some(64 * MIB - 256 * KIB)
        ]
    );
    let rings: Vec<_> = labels
        .rings()
        .map(|r| (r.label, r.start, r.end()))
        .collect();
    assert_eq!(
        rings,
        [
            (0, 128 * KIB, 256 * KIB),
            (1, 384 * KIB, 512 * KIB),
            (2, 64 * MIB - 384 * KIB, 64 * MIB - 256 * KIB),
            (3, 64 * MIB - 128 * KIB, 64 * MIB),
        ]
    );
    assert_eq!(labels.label_offset(LABELS), None);
    // A size that is not a whole number of labels: rounded down.
    let ragged = Labels::for_size(64 * MIB + 100 * KIB);
    assert_eq!(ragged.psize(), 64 * MIB);
    assert_eq!(ragged.label_offset(3), Some(64 * MIB - 256 * KIB));
    // The smallest volume with four labels, and one byte less.
    assert_eq!(Labels::for_size(MIB).rings().count(), 4);
    assert_eq!(Labels::for_size(MIB).label_offset(2), Some(512 * KIB));
    assert_eq!(Labels::for_size(MIB - 1).rings().count(), 0);
    assert!(Labels::for_size(MIB - 1)
        .uberblocks_in(0, &[0; 4096])
        .is_empty());
}

/// A slot written alone, in each ring, at its first, a middle and its
/// last slot, for each size of slot (1, 4 and 8 KiB: `ashift` 10 or
/// less, 12, 13 or more) and in either byte order: found once, with its
/// place, label, byte order, group and root block pointer.
#[test]
fn an_uberblock_write_is_found_in_each_ring_in_either_byte_order() {
    let labels = Labels::for_size(64 * MIB);
    for size in [SLOT_MIN, 4 * KIB, SLOT_MAX] {
        let slots = RING_SIZE / size;
        for ring in labels.rings() {
            for n in [0, slots / 2, slots - 1] {
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let at = ring.start + n * size;
                    let found = labels.uberblocks_in(at, &slot(order, 77 + n, n, size));
                    assert_eq!(
                        found,
                        [Uberblock {
                            at,
                            label: ring.label,
                            order,
                            state: state(77 + n, n),
                            rank: Rank {
                                txg: 77 + n,
                                timestamp: 1_763_000_000 + 77 + n,
                                seq: 0,
                            },
                            // No checksum of its own: the slot ends in
                            // the magic, not in a `zio_eck_t`.
                            slot: None,
                        }],
                        "slot {n} of {size} bytes in label {} ({order:?})",
                        ring.label
                    );
                    assert!(found[0].counts() && !found[0].verified());
                }
            }
        }
    }
}

/// A new label's ring, written whole (`vdev_label_init`): one uberblock
/// at its start, group 0, which is found and does not count.
#[test]
fn a_new_labels_ring_carries_group_zero_which_does_not_count() {
    let labels = Labels::for_size(64 * MIB);
    for ring in labels.rings() {
        let mut whole = vec![0; RING_SIZE as usize];
        let first = slot(ByteOrder::Little, 0, 1, SLOT_MIN);
        whole[..READ_LEN].copy_from_slice(&first[..READ_LEN]);
        let found = labels.uberblocks_in(ring.start, &whole);
        assert_eq!(found.len(), 1, "label {}", ring.label);
        assert_eq!(found[0].state.txg, 0);
        assert!(!found[0].counts());
    }
}

/// A ring written whole with an uberblock in every slot, for each size
/// of slot: every one is found, in order, and nothing between them, the
/// checksums at the slots' ends included.
#[test]
fn a_whole_ring_shows_every_slot_and_nothing_between() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().nth(2).expect("label 2");
    for size in [SLOT_MIN, 4 * KIB, SLOT_MAX] {
        let slots = RING_SIZE / size;
        let whole: Vec<u8> = (0..slots)
            .flat_map(|n| slot(ByteOrder::Big, 1000 + n, n, size))
            .collect();
        let found = labels.uberblocks_in(ring.start, &whole);
        let txgs: Vec<_> = found.iter().map(|u| u.state.txg).collect();
        let wanted: Vec<_> = (0..slots).map(|n| 1000 + n).collect();
        assert_eq!(txgs, wanted, "slots of {size} bytes");
        assert!(found
            .iter()
            .zip(0..)
            .all(|(u, n)| u.at == ring.start + n * size && u.label == 2));
    }
}

/// Writes that reach no ring carry no uberblock, even with the magic in
/// them: the padding, the boot block, the configuration, the space
/// between the labels, the end of the vdev past the last label's ring.
#[test]
fn writes_outside_the_rings_carry_none() {
    let size = 64 * MIB;
    let labels = Labels::for_size(size);
    let one = slot(ByteOrder::Little, 5, 5, 4 * KIB);
    for at in [
        0,
        8 * KIB,
        16 * KIB,
        128 * KIB - 4 * KIB,
        256 * KIB,
        MIB,
        32 * MIB,
        size - 512 * KIB,
        size - 384 * KIB - 4 * KIB,
    ] {
        assert!(!labels.reaches_a_ring(at, one.len() as u64), "at {at}");
        assert!(labels.uberblocks_in(at, &one).is_empty(), "at {at}");
    }
    assert!(labels.reaches_a_ring(128 * KIB - 4 * KIB, 4 * KIB + 1));
    assert!(labels.reaches_a_ring(size - 4 * KIB, 4 * KIB));
}

/// A write that crosses a ring's edge: from the configuration into the
/// ring's first slot, and from the ring's last slot into the next
/// label. What lies in the ring is found; what lies outside is not.
#[test]
fn a_write_across_a_rings_edge_shows_what_lies_inside() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    // Before the ring: a slot's worth of the magic, which is not in a
    // ring and is not read.
    let mut across = slot(ByteOrder::Little, 9, 9, 4 * KIB);
    across.extend(slot(ByteOrder::Little, 10, 10, 4 * KIB));
    let found = labels.uberblocks_in(ring.start - 4 * KIB, &across);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].at, found[0].state.txg), (ring.start, 10));
    // From the last slot into label 1's padding.
    let found = labels.uberblocks_in(ring.end() - 4 * KIB, &across);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].at, found[0].state.txg), (ring.end() - 4 * KIB, 9));
}

/// A write that holds fewer than [`READ_LEN`] bytes of a slot from its
/// start is not read, and one that begins after a slot's start does not
/// show that slot.
#[test]
fn a_write_that_holds_too_little_of_a_slot_is_not_read() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().nth(1).expect("label 1");
    let one = slot(ByteOrder::Little, 3, 3, 4 * KIB);
    assert_eq!(labels.uberblocks_in(ring.start, &one[..READ_LEN]).len(), 1);
    assert!(labels
        .uberblocks_in(ring.start, &one[..READ_LEN - 1])
        .is_empty());
    assert!(labels
        .uberblocks_in(ring.start + 512, &one[512..])
        .is_empty());
}

/// In a big-endian pool every word is read big endian, and a
/// little-endian reading of it is never taken for an uberblock.
#[test]
fn the_byte_order_is_the_magics() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let txg = 0x0102_0304_0506_0708;
    let found = labels.uberblocks_in(ring.start, &slot(ByteOrder::Big, txg, 4, 4 * KIB));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].order, ByteOrder::Big);
    assert_eq!(found[0].state, state(txg, 4));
    // A word near the magic, in either byte order, is not it.
    for word in [
        (MAGIC + 1).to_le_bytes(),
        (MAGIC + 1).to_be_bytes(),
        (MAGIC << 8).to_le_bytes(),
    ] {
        let mut noise = slot(ByteOrder::Little, 5, 5, 4 * KIB);
        noise[..8].copy_from_slice(&word);
        assert!(labels.uberblocks_in(ring.start, &noise).is_empty());
    }
}

/// §5.2's repeat: the same group and root block pointer as the
/// uberblock that marks the cut to come, or as the newest cut. A
/// multihost copy (the same group and root, another timestamp and
/// multihost fields) of either is one; a rewind's new root under a group
/// already used is not; nor is anything where there is no mark and no
/// cut.
#[test]
fn a_repeat_is_the_same_group_and_root_as_the_mark_or_the_newest_cut() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().nth(3).expect("label 3");
    let synced = labels.uberblocks_in(ring.start, &slot(ByteOrder::Little, 40, 7, 4 * KIB))[0];
    // Multihost's copy: the same uberblock, its timestamp and multihost
    // fields changed, in the ring's last slot.
    let mut copy = slot(ByteOrder::Little, 40, 7, 4 * KIB);
    copy[32..40].copy_from_slice(&1_999_999_999u64.to_le_bytes());
    copy[MMP_CONFIG_AT..MMP_CONFIG_AT + 8].copy_from_slice(&123u64.to_le_bytes());
    let mmp = labels.uberblocks_in(ring.end() - 4 * KIB, &copy)[0];
    assert!(is_repeat(Some(&synced.state), None, &mmp.state));
    assert!(is_repeat(None, Some(&synced.state), &mmp.state));
    // A stale copy of the group before, after the mark of the next:
    // the newest cut's, and so a repeat.
    let (before, next) = (state(39, 6), state(40, 7));
    assert!(is_repeat(Some(&next), Some(&before), &before));
    assert!(is_repeat(Some(&next), Some(&before), &next));
    assert!(
        !is_repeat(Some(&state(40, 7)), Some(&state(39, 6)), &state(40, 8)),
        "a rewind's new root"
    );
    assert!(
        !is_repeat(None, Some(&state(40, 7)), &state(40, 8)),
        "a rewind's new root"
    );
    assert!(!is_repeat(Some(&state(40, 7)), None, &state(41, 7)));
    assert!(!is_repeat(None, Some(&state(39, 6)), &state(38, 6)));
    assert!(!is_repeat(None, None, &state(40, 7)));
}

/// `s` with its label checksum, as ZFS writes it at `at` (little
/// endian).
fn sealed(s: &[u8], at: u64) -> Vec<u8> {
    let mut s = s.to_vec();
    seal(&mut s, at, ByteOrder::Little);
    s
}

/// A slot like [`slot`], of 4 KiB, with its timestamp and its multihost
/// config set.
fn slot_at(txg: u64, root: u64, timestamp: u64, config: u64) -> Vec<u8> {
    let mut s = slot(ByteOrder::Little, txg, root, 4 * KIB);
    s[TIMESTAMP_AT..TIMESTAMP_AT + 8].copy_from_slice(&timestamp.to_le_bytes());
    s[MMP_CONFIG_AT..MMP_CONFIG_AT + 8].copy_from_slice(&config.to_le_bytes());
    s
}

/// ZFS's order among uberblocks (`vdev_uberblock_compare`): the group,
/// then the timestamp, then the multihost sequence, which counts only
/// with `MMP_MAGIC` and the sequence's valid bit, in either byte order.
#[test]
fn the_rank_is_the_group_then_the_timestamp_then_the_multihost_sequence() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let rank = |s: &[u8]| labels.uberblocks_in(ring.start, s)[0].rank;
    let seq = |n: u64| (n << 32) | MMP_SEQ_VALID_BIT;
    assert_eq!(
        rank(&slot_at(9, 1, 100, seq(7))),
        Rank {
            txg: 9,
            timestamp: 100,
            seq: 7
        }
    );
    // Without the valid bit, or without the magic, the sequence is 0.
    assert_eq!(rank(&slot_at(9, 1, 100, 7 << 32)).seq, 0);
    let mut no_magic = slot_at(9, 1, 100, seq(7));
    no_magic[MMP_MAGIC_AT..MMP_MAGIC_AT + 8].copy_from_slice(&0u64.to_le_bytes());
    assert_eq!(rank(&no_magic).seq, 0);
    // Big endian reads the same.
    let mut big = slot(ByteOrder::Big, 9, 1, 4 * KIB);
    big[TIMESTAMP_AT..TIMESTAMP_AT + 8].copy_from_slice(&100u64.to_be_bytes());
    big[MMP_CONFIG_AT..MMP_CONFIG_AT + 8].copy_from_slice(&seq(7).to_be_bytes());
    assert_eq!(rank(&big), rank(&slot_at(9, 1, 100, seq(7))));
    // The order: a group outranks any timestamp, a timestamp any
    // sequence.
    assert!(rank(&slot_at(10, 1, 1, 0)) > rank(&slot_at(9, 1, 999, seq(9))));
    assert!(rank(&slot_at(9, 1, 101, 0)) > rank(&slot_at(9, 1, 100, seq(9))));
    assert!(rank(&slot_at(9, 1, 100, seq(2))) > rank(&slot_at(9, 1, 100, seq(1))));
}

/// The view chooses as an import does: of a volume whose rings are not
/// known, nothing; then the state whose uberblock is greatest. After a
/// rewind the abandoned history's higher group is chosen over the new
/// history's, until the new history writes that group again with a
/// later timestamp. A multihost copy of the same state with a later
/// timestamp opens that state. Two states equal by the order: neither.
/// The group alone for the order, the defect, chooses neither of the
/// two states written under one group.
#[test]
fn the_view_chooses_the_state_an_import_opens() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let mut view = View::unknown(labels);
    assert!(!view.is_known());
    assert!(!view.chosen(&state(0, 0), false));
    for ring in &rings {
        view.saw(ring.start, &vec![0; RING_SIZE as usize]);
    }
    assert!(view.is_known());
    assert_eq!(view.uberblocks().count(), 0);
    let put = |view: &mut View, slot: u64, s: &[u8]| {
        for ring in &rings {
            let at = ring.start + slot * 4 * KIB;
            view.saw(at, &sealed(s, at));
        }
    };
    // The abandoned history: 51 at slot 19.
    put(&mut view, 19, &slot_at(51, 51, 1000, 0));
    assert!(view.chosen(&state(51, 51), false));
    // A rewind to 37; the new history writes 39 at slot 7: 51 is still
    // chosen.
    put(&mut view, 7, &slot_at(39, 390, 2000, 0));
    assert!(!view.chosen(&state(39, 390), false));
    assert!(view.chosen(&state(51, 51), false));
    // The new history's own 51, later: chosen. By the group alone,
    // neither 51 is.
    put(&mut view, 3, &slot_at(51, 510, 3000, 0));
    assert!(view.chosen(&state(51, 510), false));
    assert!(!view.chosen(&state(51, 51), false));
    assert!(!view.chosen(&state(51, 510), true), "the defect");
    // A multihost copy of the same state, later: the same state opens.
    put(&mut view, 31, &slot_at(51, 510, 3005, (4 << 32) | 2));
    assert!(view.chosen(&state(51, 510), false));
    // Another state equal by the order: neither is chosen.
    let rings0 = rings[0];
    let at = rings0.start + 5 * 4 * KIB;
    view.saw(at, &sealed(&slot_at(51, 777, 3005, (4 << 32) | 2), at));
    assert!(!view.chosen(&state(51, 510), false));
    assert!(!view.chosen(&state(51, 777), false));
    // Zeroed, it is gone.
    view.zeroed(rings0.start + 5 * 4 * KIB, 4 * KIB);
    assert!(view.chosen(&state(51, 510), false));
}

/// A position covered only in part is not known: what it holds then is
/// not known. Covered whole again, it is. A held slot changed past the
/// fields read, but within the slot, is not known either: its checksum
/// covers those bytes (the review of #96, its S1). Bytes past the slot
/// leave it as it is.
#[test]
fn a_position_covered_in_part_is_not_known() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let mut view = View::empty(labels);
    let nine = sealed(&slot_at(9, 1, 100, 0), ring.start);
    view.saw(ring.start, &nine);
    assert!(view.chosen(&state(9, 1), false));
    // Bytes 16 to 32 of the slot: its group.
    view.saw(ring.start + 16, &[0; 16]);
    assert!(!view.is_known());
    assert!(!view.chosen(&state(9, 1), false));
    // Bytes after the fields read, within the slot: not known.
    view.saw(ring.start, &nine);
    view.saw(ring.start + READ_LEN as u64, &[0; 64]);
    assert!(!view.is_known());
    // Bytes past the slot leave the position as it is.
    view.saw(ring.start, &nine);
    view.saw(ring.start + 4 * KIB, &[0; KIB as usize]);
    assert!(view.is_known() && view.chosen(&state(9, 1), false));
    // Zeros over part of it: not known.
    view.zeroed(ring.start + 8, 8);
    assert!(!view.is_known());
}

/// A slot sealed as ZFS writes it, at each size ZFS uses, in each ring
/// and in either byte order, is verified at its own size and no other.
/// It is not verified when a byte of it changes, when its checksum was
/// made for another place (a slot copied elsewhere), or when the write
/// holds only part of it.
#[test]
fn an_uberblock_is_verified_by_its_label_checksum() {
    let labels = Labels::for_size(64 * MIB);
    for size in SLOT_SIZES {
        let slots = RING_SIZE / size;
        for ring in labels.rings() {
            for n in [1, slots - 1] {
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let at = ring.start + n * size;
                    let mut s = slot(order, 77 + n, n, size);
                    seal(&mut s, at, order);
                    assert!(label_checksum_holds(&s, at));
                    let found = labels.uberblocks_in(at, &s);
                    assert_eq!(found.len(), 1);
                    assert_eq!(
                        found[0].slot,
                        Some(size),
                        "slot {n} of {size} bytes in label {} ({order:?})",
                        ring.label
                    );
                    // Another place.
                    assert!(!label_checksum_holds(&s, at + size));
                    let mut elsewhere = slot(order, 77 + n, n, size);
                    seal(&mut elsewhere, at - size, order);
                    assert!(!labels.uberblocks_in(at, &elsewhere)[0].verified());
                    // A byte changed: the root block pointer's.
                    let mut changed = s.clone();
                    changed[ROOT_AT + 3] ^= 1;
                    assert!(!labels.uberblocks_in(at, &changed)[0].verified());
                    // Part of the slot.
                    let part = &s[..size as usize / 2];
                    if part.len() >= READ_LEN {
                        assert!(!labels.uberblocks_in(at, part)[0].verified());
                    }
                }
            }
        }
    }
}

/// The checksum's words follow the byte order its magic shows: a slot
/// sealed big endian holds, and the same slot with its four words
/// written little endian, under the same big-endian magic, does not.
/// (The magic itself is hashed, so changing it would break the checksum
/// in either order and show nothing of the order: the review of #96,
/// its S3.)
#[test]
fn the_checksum_is_read_in_the_order_its_magic_shows() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let at = ring.start + 4 * KIB;
    let mut s = slot(ByteOrder::Big, 9, 1, 4 * KIB);
    seal(&mut s, at, ByteOrder::Big);
    assert!(label_checksum_holds(&s, at));
    let eck = s.len() - ECK_LEN;
    for n in 0..4 {
        let to = eck + 8 + 8 * n;
        let mut word = [0; 8];
        word.copy_from_slice(&s[to..to + 8]);
        s[to..to + 8].copy_from_slice(&u64::from_be_bytes(word).to_le_bytes());
    }
    assert!(!label_checksum_holds(&s, at));
}

/// What a new label writes, the whole ring at once with an uberblock of
/// group 0 at its start, carries no checksum of the slot's own: it is
/// found and not verified.
#[test]
fn a_new_labels_ring_is_not_verified() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let mut whole = vec![0; RING_SIZE as usize];
    whole[..SLOT_MIN as usize].copy_from_slice(&slot(ByteOrder::Little, 0, 0, SLOT_MIN));
    let found = labels.uberblocks_in(ring.start, &whole);
    assert_eq!(found.len(), 1);
    assert!(!found[0].counts() && !found[0].verified());
}

/// While a ring is not known, no state is chosen, though the rings
/// that are known hold one uberblock and nothing above it (the review
/// of #95, its S2, whose reproducer this is). Fails where
/// `View::chosen` does not ask `is_known()` (plant V1), and where
/// `View::unknown` covers the first ring alone (plant V13).
#[test]
fn a_ring_not_known_leaves_no_state_chosen() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let mut view = View::unknown(labels);
    // The start read three rings; the last could not be read.
    for ring in &rings[..3] {
        view.saw(ring.start, &vec![0; RING_SIZE as usize]);
    }
    // Sealed, as M2.1c counts only a slot whose label checksum holds.
    for ring in &rings[..3] {
        let at = ring.start + 9 * 4 * KIB;
        view.saw(at, &sealed(&slot_at(9, 1, 100, 0), at));
    }
    assert!(!view.is_known());
    assert!(!view.chosen(&state(9, 1), false));
    // Once the last ring is known too, the state is chosen.
    view.saw(rings[3].start, &vec![0; RING_SIZE as usize]);
    let at = rings[3].start + 9 * 4 * KIB;
    view.saw(at, &sealed(&slot_at(9, 1, 100, 0), at));
    assert!(view.is_known() && view.chosen(&state(9, 1), false));
}

/// The label checksum of one slot as ZFS computes it, with answers that
/// do not come from this crate: they were computed from the OpenZFS
/// 2.4.1 sources (`zio_checksum_compute` with `ZIO_CHECKSUM_LABEL`,
/// `abd_checksum_sha256`) with Python's hashlib. The slot is 1 KiB at
/// 0x20400, the uberblock magic and group 7 with zeros after, as a
/// little-endian and as a big-endian host writes it. `seal` and
/// `label_checksum_holds` share `label_checksum`, so a slot the tests
/// seal themselves cannot show an error both would make: for a
/// big-endian writer, of which there is no recording, this is the one
/// test that could. From the review of #96, its S2 (points 2 and 3).
#[test]
fn the_label_checksum_gives_the_answers_zfs_gives() {
    const AT: u64 = 0x20400;
    let cases = [
        (
            ByteOrder::Little,
            "6e7c41876ec86b3c96c86ae658f2957456e6c4ad5cdd7c5670dab273fb616b2d",
        ),
        (
            ByteOrder::Big,
            "021d60e737324c57bd910abcd993623e8e838e506fe22bc488c8ac6dbca378f9",
        ),
    ];
    for (order, last) in cases {
        let mut s = vec![0u8; SLOT_MIN as usize];
        s[..8].copy_from_slice(&order.bytes(MAGIC));
        s[16..24].copy_from_slice(&order.bytes(7));
        seal(&mut s, AT, order);
        let words: String = s[s.len() - 32..]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(words, last, "{order:?}");
        assert!(label_checksum_holds(&s, AT), "{order:?}");
        // Each of the four words counts, and so does the checksum's
        // magic, which is hashed.
        for at in [8, 16, 24, 32, 40] {
            let mut changed = s.clone();
            let byte = changed.len() - at;
            changed[byte] ^= 1;
            assert!(!label_checksum_holds(&changed, AT), "{order:?}, {at}");
        }
    }
}

/// A slot whose checksum holds, at a place where no slot of its size
/// begins, is not verified: ZFS reads a ring only at whole slots of the
/// pool's size. From the review of #96, its S2.
#[test]
fn a_slot_out_of_step_with_the_ring_is_not_verified() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    for (size, into) in [
        (2 * KIB, KIB),
        (4 * KIB, KIB),
        (4 * KIB, 2 * KIB),
        (8 * KIB, 4 * KIB),
    ] {
        let at = ring.start + size + into;
        let mut s = slot(ByteOrder::Little, 9, 1, size);
        seal(&mut s, at, ByteOrder::Little);
        assert!(label_checksum_holds(&s, at));
        let found = labels.uberblocks_in(at, &s);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slot, None, "{size} bytes, {into} into a slot");
    }
}

/// A slot whose checksum carries no magic is held against its words as
/// a little-endian host reads them: `zio_checksum_error_impl` asks only
/// whether the magic reads swapped, and takes any other for its own
/// order. The magic is hashed, so the words are those of the slot with
/// that magic in place. From the review of #96, its S2.
#[test]
fn a_checksum_without_its_magic_is_read_as_a_little_endian_host_reads_it() {
    let labels = Labels::for_size(64 * MIB);
    let ring = labels.rings().next().expect("label 0");
    let at = ring.start + 4 * KIB;
    let mut s = slot(ByteOrder::Little, 9, 1, 4 * KIB);
    seal(&mut s, at, ByteOrder::Little);
    let eck = s.len() - ECK_LEN;
    s[eck..eck + 8].copy_from_slice(&[0x55; 8]);
    assert!(!label_checksum_holds(&s, at));
    let words = label_checksum(&s, at, ByteOrder::Little);
    for (n, word) in words.into_iter().enumerate() {
        let to = eck + 8 + 8 * n;
        s[to..to + 8].copy_from_slice(&word.to_le_bytes());
    }
    assert!(label_checksum_holds(&s, at));
    assert_eq!(labels.uberblocks_in(at, &s)[0].slot, Some(4 * KIB));
}

/// The view holds only what ZFS takes for an uberblock: a slot with the
/// magic and a group whose checksum does not hold leaves its place
/// known and empty, and the same slot sealed is held. From the review
/// of #96, its S2.
#[test]
fn the_view_holds_no_uberblock_whose_checksum_fails() {
    let labels = Labels::for_size(64 * MIB);
    let mut view = View::empty(labels);
    for ring in labels.rings() {
        view.saw(ring.start, &vec![0; RING_SIZE as usize]);
    }
    let ring = labels.rings().next().expect("label 0");
    let at = ring.start + 4 * KIB;
    let unsealed = slot_at(9, 1, 100, 0);
    view.saw(at, &unsealed);
    assert!(view.is_known());
    assert_eq!(view.uberblocks().count(), 0);
    assert!(!view.chosen(&state(9, 1), false));
    view.saw(at, &sealed(&unsealed, at));
    assert!(view.chosen(&state(9, 1), false));
}

/// An uberblock is valid by its whole slot, so a change to any of the
/// slot counts (the review of #96, its S1, whose test this is, and
/// Codex's finding there). (1) A held 8 KiB slot whose second half is
/// zeroed no longer verifies on the volume: the view does not go on
/// choosing it. (2) The same slot arriving in two writes, head then
/// tail, is whole and valid on the volume: the view does not call its
/// place known and empty. Both fail where the view follows a slot by its
/// first `READ_LEN` bytes alone, as at 8da0a73.
#[test]
fn a_change_to_any_part_of_a_slot_counts() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let size = 8 * KIB;
    let at = rings[0].start + size;
    let mut s = slot(ByteOrder::Little, 9, 1, size);
    seal(&mut s, at, ByteOrder::Little);
    let read = |view: &mut View| {
        for ring in &rings {
            view.saw(ring.start, &vec![0; RING_SIZE as usize]);
        }
    };
    // (1)
    let mut view = View::empty(labels);
    read(&mut view);
    view.saw(at, &s);
    assert!(view.is_known() && view.chosen(&state(9, 1), false));
    view.zeroed(at + 4 * KIB, 4 * KIB);
    let mut on_disk = s.clone();
    on_disk[4 * KIB as usize..].fill(0);
    assert!(!label_checksum_holds(&on_disk, at));
    assert!(!view.chosen(&state(9, 1), false));
    // Written whole again, it is held again.
    view.saw(at, &s);
    assert!(view.is_known() && view.chosen(&state(9, 1), false));
    // (2)
    let mut view = View::empty(labels);
    read(&mut view);
    view.saw(at, &s[..4 * KIB as usize]);
    view.saw(at + 4 * KIB, &s[4 * KIB as usize..]);
    assert!(label_checksum_holds(&s, at));
    assert!(!view.is_known() || view.chosen(&state(9, 1), false));
}

/// What ZFS writes leaves every place known: a slot of 4 KiB written
/// whole where a slot of 8 KiB could also begin, and a ring read whole.
/// The view's care for slots written in part costs ZFS's own writes
/// nothing.
#[test]
fn whole_slots_leave_every_place_known() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let mut view = View::unknown(labels);
    let mut ring = vec![0; RING_SIZE as usize];
    let at = rings[0].start + 8 * KIB;
    let mut s = slot(ByteOrder::Little, 9, 1, 4 * KIB);
    seal(&mut s, at, ByteOrder::Little);
    ring[8 * KIB as usize..12 * KIB as usize].copy_from_slice(&s);
    view.saw(rings[0].start, &ring);
    for ring in &rings[1..] {
        view.saw(ring.start, &vec![0; RING_SIZE as usize]);
    }
    assert!(view.is_known() && view.chosen(&state(9, 1), false));
    let at = rings[1].start + 16 * KIB;
    let mut s = slot(ByteOrder::Little, 10, 2, 4 * KIB);
    seal(&mut s, at, ByteOrder::Little);
    view.saw(at, &s);
    assert!(view.is_known() && view.chosen(&state(10, 2), false));
}
