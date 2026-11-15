// The next look at #96 (110d2b6), and Codex's second finding: append to
// crates/skyzfs-uberblock/src/tests.rs. It FAILS at 110d2b6, at both
// asserts marked below.

/// A header the volume already holds, known not to verify, is completed
/// by a later write that does not touch the header: the volume then
/// holds a valid slot, and the view must not call its place known and
/// empty. (a) The damaged slot is in a ring read at the start. (b) It
/// comes in a write that holds the whole slot. In both, the write that
/// restores the slot's second half is the one that makes it valid.
#[test]
fn a_header_known_not_to_verify_can_be_completed_by_its_tail() {
    let labels = Labels::for_size(64 * MIB);
    let rings: Vec<Ring> = labels.rings().collect();
    let size = 8 * KIB;
    let at = rings[0].start + size;
    let mut s = slot(ByteOrder::Little, 99, 9, size);
    seal(&mut s, at, ByteOrder::Little);
    assert!(label_checksum_holds(&s, at));
    let mut damaged = s.clone();
    damaged[4 * KIB as usize..].fill(0);
    let tail = &s[4 * KIB as usize..];
    // (a)
    let mut view = View::unknown(labels);
    for ring in &rings {
        let mut whole = vec![0; RING_SIZE as usize];
        if ring.label == 0 {
            whole[size as usize..2 * size as usize].copy_from_slice(&damaged);
        }
        view.saw(ring.start, &whole);
    }
    assert!(view.is_known() && !view.chosen(&state(99, 9), false));
    view.saw(at + 4 * KIB, tail);
    assert!(!view.is_known() || view.chosen(&state(99, 9), false)); // fails at 110d2b6
    // (b)
    let mut view = View::empty(labels);
    for ring in &rings {
        view.saw(ring.start, &vec![0; RING_SIZE as usize]);
    }
    view.saw(at, &damaged);
    assert!(view.is_known() && !view.chosen(&state(99, 9), false));
    view.saw(at + 4 * KIB, tail);
    assert!(!view.is_known() || view.chosen(&state(99, 9), false)); // fails at 110d2b6
}
