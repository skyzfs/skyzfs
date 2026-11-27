use super::*;

/// A note as the daemon writes it, byte for byte: the layout the daemon
/// and the agent both read (`skyzfs_ublk::records`' table). A change of
/// the layout changes this, on purpose.
fn golden() -> [u8; FAILED_LEN] {
    let mut bytes = [0u8; FAILED_LEN];
    bytes[..8].copy_from_slice(b"SKYZFAI1");
    for (i, b) in bytes[8..24].iter_mut().enumerate() {
        *b = 0x10 + i as u8;
    }
    for (i, b) in bytes[24..40].iter_mut().enumerate() {
        *b = 0xa0 + i as u8;
    }
    bytes[40] = 1;
    bytes[41] = 7;
    bytes[48..56].copy_from_slice(&123_456_u64.to_le_bytes());
    bytes
}

fn stored() -> StoredFailure {
    let mut boot = [0u8; 16];
    let mut identity = [0u8; 16];
    for i in 0..16 {
        boot[i] = 0x10 + i as u8;
        identity[i] = 0xa0 + i as u8;
    }
    StoredFailure {
        boot,
        failure: Failure {
            held: Held::Yes,
            reason: Reason::ImageFailed,
            identity,
        },
        written_ms: 123_456,
    }
}

#[test]
fn the_note_has_the_daemons_layout() {
    assert_eq!(encode_failure(&stored()), golden());
    assert_eq!(decode_failure(&golden()), Some(stored()));
}

#[test]
fn every_code_and_holding_reads_back() {
    let reasons = [
        (1, Reason::LogFailed),
        (2, Reason::Short),
        (3, Reason::RecoveryFailed),
        (4, Reason::Marked),
        (5, Reason::PositionUnreadable),
        (6, Reason::DeviceNoteUnreadable),
        (7, Reason::ImageFailed),
    ];
    for (code, reason) in reasons {
        for (byte, held) in [(0, Held::No), (1, Held::Yes), (2, Held::Unknown)] {
            let mut bytes = golden();
            bytes[40] = byte;
            bytes[41] = code;
            let note = decode_failure(&bytes).expect("a whole note");
            assert_eq!((note.failure.reason, note.failure.held), (reason, held));
            assert_eq!(encode_failure(&note), bytes);
        }
    }
}

#[test]
fn a_note_that_is_not_whole_does_not_decode() {
    type Spoil = fn(&mut Vec<u8>);
    let cases: [(&str, Spoil); 8] = [
        ("short", |b| {
            b.pop();
        }),
        ("long", |b| b.push(0)),
        ("magic", |b| b[7] = b'2'),
        ("holding 3", |b| b[40] = 3),
        ("code 0", |b| b[41] = 0),
        ("code 8", |b| b[41] = 8),
        ("padding at 42", |b| b[42] = 1),
        ("padding at 56", |b| b[63] = 1),
    ];
    for (name, spoil) in cases {
        let mut bytes = golden().to_vec();
        spoil(&mut bytes);
        assert_eq!(decode_failure(&bytes), None, "{name}");
    }
}

#[test]
fn reading_tells_none_a_note_and_one_that_is_not_whole() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join(FAILED);
    assert!(read_failure(&path).expect("read").is_none());
    std::fs::write(&path, golden()).expect("write");
    assert_eq!(read_failure(&path).expect("read"), Some(stored()));
    std::fs::write(&path, &golden()[..63]).expect("write");
    let e = read_failure(&path).expect_err("not whole");
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

// Moved here with the code from `skyzfs_ublk::records` (M2.7c).
#[test]
fn the_boot_id_is_the_kernels_uuid() {
    assert_eq!(
        parse_boot_id("00010203-0405-0607-0809-0a0b0c0d0e0f\n"),
        Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
    );
    for wrong in [
        "",
        "0001",
        "00010203-0405-0607-0809-0a0b0c0d0e0g",
        "+0010203-0405-0607-0809-0a0b0c0d0e0f",
    ] {
        assert_eq!(parse_boot_id(wrong), None, "{wrong:?}");
    }
    if cfg!(target_os = "linux") {
        assert_eq!(boot_id().expect("boot id"), boot_id().expect("boot id"));
    }
}

fn failure(held: bool, reason: Reason) -> Failure {
    Failure {
        held: held.into(),
        reason,
        identity: [3; 16],
    }
}

#[test]
fn the_failure_note_has_the_documented_layout() {
    let stored = StoredFailure {
        boot: [7; 16],
        failure: failure(true, Reason::RecoveryFailed),
        written_ms: 0x0102_0304_0506_0708,
    };
    let bytes = encode_failure(&stored);
    let mut expected = Vec::new();
    expected.extend(b"SKYZFAI1");
    expected.extend([7; 16]);
    expected.extend([3; 16]);
    expected.extend([1, 3, 0, 0, 0, 0, 0, 0]);
    expected.extend([8, 7, 6, 5, 4, 3, 2, 1]);
    expected.extend([0; 8]);
    assert_eq!(bytes.to_vec(), expected);
    assert_eq!(decode_failure(&bytes), Some(stored));

    // Every reason has its own code and comes back.
    let mut codes = Vec::new();
    for (code, reason) in REASONS {
        for held in [false, true] {
            let stored = StoredFailure {
                failure: failure(held, reason),
                ..stored
            };
            let bytes = encode_failure(&stored);
            assert_eq!((bytes[40], bytes[41]), (u8::from(held), code));
            assert_eq!(decode_failure(&bytes), Some(stored));
        }
        codes.push(code);
    }
    codes.dedup();
    assert_eq!(codes, [1, 2, 3, 4, 5, 6, 7]);
    // The in-place store's reason is code 7, and the log store's keeps
    // code 1: notes written before code 7 existed read as they did.
    for (reason, code) in [(Reason::LogFailed, 1), (Reason::ImageFailed, 7)] {
        let stored = StoredFailure {
            failure: failure(false, reason),
            ..stored
        };
        assert_eq!(encode_failure(&stored)[41], code, "{reason:?}");
    }
    // A code this version does not know is not a whole note: the start
    // refuses the volume as failed while held, as a daemon older than
    // code 7 does with a note of code 7.
    let mut unknown = encode_failure(&stored);
    unknown[41] = 8;
    assert_eq!(decode_failure(&unknown), None);

    // Not known whether a client held the device: its own value.
    let unknown = StoredFailure {
        failure: Failure {
            held: Held::Unknown,
            ..stored.failure
        },
        ..stored
    };
    let bytes = encode_failure(&unknown);
    assert_eq!(bytes[40], 2);
    assert_eq!(decode_failure(&bytes), Some(unknown));
}

/// Each reason and holding has a word of its own (the agent's `lost`
/// line). Mutant: two reasons with one word.
#[test]
fn every_reason_has_its_own_word() {
    let mut words: Vec<_> = REASONS.iter().map(|(_, r)| r.word()).collect();
    assert_eq!(words[6], "image-failed");
    words.sort_unstable();
    words.dedup();
    assert_eq!(words.len(), REASONS.len());
    assert_eq!(
        [Held::No.word(), Held::Yes.word(), Held::Unknown.word()],
        ["no", "yes", "unknown"]
    );
}
