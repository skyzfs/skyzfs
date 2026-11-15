// S1 of the review of #96: append to crates/skyzfs-uberblock/src/tests.rs.
// It FAILS at 8da0a73, at both asserts marked below: it states what the
// view would say if it followed a slot by all of its bytes.

/// An uberblock is valid by its whole slot, so a change to any of the
/// slot counts. (1) A held 8 KiB slot whose second half is zeroed no
/// longer verifies on the volume: the view must not go on choosing it.
/// (2) The same slot arriving in two writes, head then tail, is whole
/// and valid on the volume: the view must not call its place known and
/// empty.
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
    assert!(!view.chosen(&state(9, 1), false)); // fails at 8da0a73
    // (2)
    let mut view = View::empty(labels);
    read(&mut view);
    view.saw(at, &s[..4 * KIB as usize]);
    view.saw(at + 4 * KIB, &s[4 * KIB as usize..]);
    assert!(label_checksum_holds(&s, at));
    assert!(!view.is_known() || view.chosen(&state(9, 1), false)); // fails at 8da0a73
}
