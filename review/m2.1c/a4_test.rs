// #96 at 2c7a3ce: a test for the one plant on the new rule that passes every test (A4 in
// plants3.py: a remembered place whose fields are written again stays remembered).
// Append to crates/skyzfs-uberblock/src/tests.rs. It passes at 2c7a3ce and fails under the plant.

/// A place remembered for fields that did not verify is forgotten once
/// its fields are written again: ZFS writes that slot whole, and then
/// the slot after it, and every place stays known. Each ring is first
/// written whole with the fields of group 99 at 8 KiB, where a slot of
/// 8 KiB could begin, and no slot there verifies. Fails where the place
/// stays remembered: the write of the next 4 KiB slot then reaches the
/// 8 KiB that was remembered, and the place of a slot ZFS has just
/// written whole reads unknown.
#[test]
fn a_place_written_whole_again_is_no_longer_followed() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let mut view = View::empty(labels);
    for ring in &rings {
        let mut whole = vec![0; RING_SIZE as usize];
        whole[8 * KIB as usize..12 * KIB as usize]
            .copy_from_slice(&slot(ByteOrder::Little, 99, 9, 4 * KIB));
        view.saw(ring.start, &whole);
    }
    assert!(view.is_known() && !view.chosen(&state(99, 9), false));
    for ring in &rings {
        let at = ring.start + 8 * KIB;
        view.saw(at, &sealed(&slot_at(5, 1, 100, 0), at));
    }
    assert!(view.is_known() && view.chosen(&state(5, 1), false));
    for ring in &rings {
        let at = ring.start + 12 * KIB;
        view.saw(at, &sealed(&slot_at(6, 2, 101, 0), at));
    }
    assert!(view.is_known() && view.chosen(&state(6, 2), false));
}
