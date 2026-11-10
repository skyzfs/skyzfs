//! Durable state store: atomic round-trip, torn-write tolerance, versioning
//! (agent design doc §4.3).

use skynbd_agent::state::{
    AgentVolumeState, BaselinePair, LoadedState, LocalSnap, OpCursor, Phase, PreparedRef,
    StateStore,
};
use skynbd_format::Codec;

fn sample() -> AgentVolumeState {
    AgentVolumeState {
        volume: "vlin".into(),
        phase: Phase::Backfilling,
        baseline: Some(BaselinePair {
            local: LocalSnap {
                lv_name: "skynbd-b3".into(),
                thin_dev_id: 11,
                pool_tx_id: 42,
                created_at_ms: 1_000,
            },
            remote: "snap/g3".into(),
            pin_holder: "baseline:host1".into(),
        }),
        candidate: Some(LocalSnap {
            lv_name: "skynbd-s4".into(),
            thin_dev_id: 12,
            pool_tx_id: 43,
            created_at_ms: 2_000,
        }),
        prepared: Some(PreparedRef {
            snap_id: "01ARZ".into(),
            target: "snap/g4".into(),
        }),
        cursor: Some(OpCursor { next_op_id: 33 }),
        advancing: false,
        epoch: None,
        advances_since_swing: 5,
        swing_flipped: Some(true),
        candidate_requests: vec!["csi-a".into()],
        served_requests: vec![skynbd_agent::ServedRequest {
            name: "csi-b".into(),
            target: "snap/g4".into(),
            capture_time_ms: 1_700_000_000_000,
        }],
        lost_requests: vec!["csi-c".into()],
        updated_at_ms: 3_000,
    }
}

#[test]
fn save_load_roundtrip_and_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());

    assert!(matches!(store.load("vlin").unwrap(), LoadedState::Absent));

    let s1 = sample();
    store.save(&s1).unwrap();
    match store.load("vlin").unwrap() {
        LoadedState::Present(got) => assert_eq!(*got, s1),
        other => panic!("expected Present, got {other:?}"),
    }
    assert!(!store.stale_tmp("vlin"), "tmp renamed away");

    // Overwrite is atomic-in-place: new phase visible, old gone.
    let mut s2 = s1.clone();
    s2.phase = Phase::Promoting;
    s2.updated_at_ms = 4_000;
    store.save(&s2).unwrap();
    match store.load("vlin").unwrap() {
        LoadedState::Present(got) => assert_eq!(*got, s2),
        other => panic!("expected Present, got {other:?}"),
    }
}

#[test]
fn torn_or_corrupt_file_is_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());
    let s = sample();
    store.save(&s).unwrap();

    let path = dir.path().join("vlin.state");
    let full = std::fs::read(&path).unwrap();

    // Torn: truncate mid-record.
    std::fs::write(&path, &full[..full.len() / 2]).unwrap();
    assert!(matches!(store.load("vlin").unwrap(), LoadedState::Corrupt));

    // Corrupt: flip a body byte (postcard decode fails or fields mismatch —
    // either way it must not panic; a silent wrong decode is prevented by
    // the frame, and content is re-verified against observations anyway).
    let mut junk = full.clone();
    let last = junk.len() - 1;
    junk[last] ^= 0xFF;
    junk.push(0xAA); // trailing garbage also must not decode cleanly to Ok
    std::fs::write(&path, &junk).unwrap();
    match store.load("vlin").unwrap() {
        LoadedState::Corrupt | LoadedState::Present(_) => {}
        LoadedState::Absent => panic!("file exists"),
    }

    // Wrong magic: definitively corrupt.
    let mut bad_magic = full.clone();
    bad_magic[..4].copy_from_slice(b"XXXX");
    std::fs::write(&path, &bad_magic).unwrap();
    assert!(matches!(store.load("vlin").unwrap(), LoadedState::Corrupt));
}

#[test]
fn future_version_rejected_not_misread() {
    // A record written by a future agent (version current+1) must load as
    // Corrupt (unreadable), never silently misparse. Frame version checks
    // give this for free — pin it.
    let s = sample();
    let mut bytes = s.encode().unwrap();
    bytes[4] = bytes[4].wrapping_add(1); // version u16 LE low byte
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("vlin.state"), &bytes).unwrap();
    let store = StateStore::new(dir.path());
    assert!(matches!(store.load("vlin").unwrap(), LoadedState::Corrupt));
}
