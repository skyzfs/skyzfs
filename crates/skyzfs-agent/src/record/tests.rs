use super::*;

pub(super) fn taken(group: u64) -> Taken {
    Taken {
        group,
        members: vec![
            Member {
                name: "tank".into(),
                snapshot_guid: 0x1111_2222_3333_4444,
            },
            Member {
                name: "tank/db".into(),
                snapshot_guid: u64::MAX,
            },
        ],
        manifest_len: 300,
        manifest_hash: [0x4d; 32],
    }
}

fn count(after: u64) -> Count {
    Count {
        attachment: [0xa1; 16],
        state: [0x5e; 16],
        after,
        load_guid: 0x10ad,
    }
}

pub(super) fn publish(name: &str, phase: Phase, held: bool) -> Publish {
    Publish {
        name: name.into(),
        count: count(3),
        phase,
        held,
        attempts: 2,
        stalls: 1,
        begun_ms: 1_700_000_000_000,
        person: None,
    }
}

/// A record with a publish in each phase.
fn full() -> Record {
    Record {
        volume: [0x0f; 16],
        enrolled: Some(Enrolled {
            pool: "tank".into(),
            pool_guid: 0x9000_0000_0000_0001,
            leaf_guid: 0x8000_0000_0000_0002,
        }),
        under_way: vec![
            publish("skz-20261125T010000Z", Phase::Intent, false),
            publish("skz-20261125T010100Z", Phase::Taken(taken(40)), false),
            publish("skz-20261125T010200Z", Phase::Held(taken(41)), true),
            publish(
                "skz-20261125T010300Z",
                Phase::Asked {
                    taken: taken(42),
                    asked_ms: 1_764_032_066_000,
                    held_until_ms: 1_764_032_706_000,
                },
                true,
            ),
            publish(
                "skz-20261125T010400Z",
                Phase::Asked {
                    taken: taken(43),
                    asked_ms: 6,
                    held_until_ms: 7,
                },
                false,
            ),
            publish(
                "skz-20261125T010500Z",
                Phase::Done {
                    taken: taken(44),
                    commit: [0xc0; 32],
                },
                true,
            ),
        ],
        abandoned: vec![Abandoned {
            name: "skz-20261125T005900Z".into(),
            why: Why::ManifestLost,
            at_ms: 5,
        }],
    }
}

/// Mutant: a field dropped or swapped, in the body or the head.
#[test]
fn a_record_reads_back_as_written() {
    for record in [Record::new([1; 16]), full()] {
        let bytes = record.encode().expect("encoded");
        assert_eq!(&bytes[..4], b"SZAG");
        assert_eq!(&bytes[4..8], &[2, 0, 0, 0]);
        assert_eq!(Record::decode(&bytes), Ok(record));
    }
    let mut record = full();
    for why in [
        Why::ManifestLost,
        Why::ManifestTooLarge,
        Why::Destroyed,
        Why::Replaced,
        Why::Premise,
        Why::NameTaken,
        Why::Rewound,
        Why::NoProgress,
        Why::TooOld,
        Why::Stopped,
        Why::Person,
        Why::NotWhole,
    ] {
        record.abandoned.push(Abandoned {
            name: format!("{why:?}"),
            why,
            at_ms: 9,
        });
    }
    let bytes = record.encode().expect("encoded");
    assert_eq!(Record::decode(&bytes), Ok(record));
}

/// Mutant: any byte of a record changed, or a record cut short, read as
/// a record (or a panic).
#[test]
fn a_record_changed_or_cut_short_is_corrupt() {
    let bytes = full().encode().expect("encoded");
    for len in 0..bytes.len() {
        assert!(Record::decode(&bytes[..len]).is_err(), "cut at {len}");
    }
    for at in 0..bytes.len() {
        for bit in [0x01, 0x80] {
            let mut changed = bytes.clone();
            changed[at] ^= bit;
            assert!(Record::decode(&changed).is_err(), "byte {at} bit {bit:#x}");
        }
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(Record::decode(&longer).is_err());
}

/// Mutant: a check of the head left out (each read past by a body that
/// is otherwise good).
#[test]
fn a_head_that_is_not_version_2s_is_named() {
    let good = full().encode().expect("encoded");
    let with = |at: usize, value: &[u8]| {
        let mut bytes = good.clone();
        bytes[at..at + value.len()].copy_from_slice(value);
        let end = bytes.len() - 32;
        let sum = blake3::hash(&bytes[..end]);
        bytes[end..].copy_from_slice(sum.as_bytes());
        Record::decode(&bytes)
    };
    assert_eq!(with(0, b"SAGT"), Err(Corrupt::Magic(*b"SAGT")));
    assert_eq!(with(4, &1u16.to_le_bytes()), Err(Corrupt::Version(1)));
    assert_eq!(with(4, &3u16.to_le_bytes()), Err(Corrupt::Version(3)));
    assert_eq!(with(4, &0u16.to_le_bytes()), Err(Corrupt::Version(0)));
    assert_eq!(with(6, &1u16.to_le_bytes()), Err(Corrupt::Flags(1)));
    let is = (good.len() - HEAD - SUM) as u64;
    assert_eq!(
        with(8, &(is as u32 + 1).to_le_bytes()),
        Err(Corrupt::Length { said: is + 1, is })
    );
    let mut sum = good.clone();
    let last = sum.len() - 1;
    sum[last] ^= 1;
    assert_eq!(Record::decode(&sum), Err(Corrupt::Hash));
    assert_eq!(Record::decode(&good[..HEAD]), Err(Corrupt::Short(HEAD)));
}

/// Mutant: a rule of the record not checked, at encode or at decode.
#[test]
fn a_record_that_breaks_a_rule_is_neither_written_nor_read() {
    let name = |i: usize| format!("skz-20261125T0100{i:02}Z");
    let mut broken = Vec::new();
    let mut nine = full();
    nine.under_way = (0..9)
        .map(|i| publish(&name(i), Phase::Intent, false))
        .collect();
    broken.push(("nine under way", nine));
    let mut twice = full();
    twice.under_way.push(twice.under_way[2].clone());
    broken.push(("a name twice", twice));
    for bad in ["", "..", ".hidden", "a/b", "a:b", "a b"] {
        let mut named = full();
        named.under_way[0].name = bad.into();
        broken.push(("a bad name", named));
    }
    let mut held = full();
    held.under_way[0].held = true;
    broken.push(("held at intent", held));
    let mut empty = full();
    empty.under_way[1].phase = Phase::Taken(Taken {
        members: Vec::new(),
        ..taken(40)
    });
    broken.push(("no members", empty));
    let mut large = full();
    large.under_way[1].phase = Phase::Taken(Taken {
        manifest_len: skyzfs_control::MANIFEST_MAX as u32 + 1,
        ..taken(40)
    });
    broken.push(("a manifest too large", large));
    let mut kept = full();
    kept.abandoned = vec![kept.abandoned[0].clone(); ABANDONED_KEPT + 1];
    broken.push(("too many given up", kept));
    for (what, record) in broken {
        assert!(
            matches!(record.encode(), Err(Corrupt::Rule(_))),
            "{what}: encoded"
        );
        let body = postcard::to_allocvec(&record).expect("a body");
        assert!(
            matches!(Record::decode(&frame(&body)), Err(Corrupt::Rule(_))),
            "{what}: decoded"
        );
    }
    // Eight under way, the most, and 64 given up, are a record.
    let mut most = full();
    most.under_way = (0..UNDER_WAY_MAX)
        .map(|i| publish(&name(i), Phase::Intent, false))
        .collect();
    most.abandoned = vec![most.abandoned[0].clone(); ABANDONED_KEPT];
    assert!(most.encode().is_ok());
}

/// Mutant: the oldest name kept in place of the newest, or none dropped.
#[test]
fn a_name_given_up_leaves_those_under_way_and_the_newest_are_kept() {
    let mut record = full();
    let name = record.under_way[3].name.clone();
    record.abandon(&name, Why::Destroyed, 77);
    assert!(record.publish(&name).is_none());
    assert_eq!(record.under_way.len(), 5);
    assert_eq!(
        record.abandoned.last(),
        Some(&Abandoned {
            name,
            why: Why::Destroyed,
            at_ms: 77
        })
    );
    for i in 0..ABANDONED_KEPT as u64 {
        record.abandon(&format!("n{i}"), Why::Premise, 100 + i);
    }
    assert_eq!(record.abandoned.len(), ABANDONED_KEPT);
    assert_eq!(record.abandoned[0].at_ms, 100);
    assert_eq!(
        record.abandoned.last().map(|a| a.at_ms),
        Some(100 + ABANDONED_KEPT as u64 - 1)
    );
}

fn store() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().expect("a directory");
    let store = Store::open(root.path().join("vol")).expect("opened");
    (root, store)
}

/// Mutant: a record not written, written where it is not read, or a
/// file that is not one read as absent.
#[test]
fn a_saved_record_loads_and_none_is_absent() {
    let (_root, store) = store();
    assert_eq!(store.load().expect("loaded"), Loaded::Absent);
    store.save(&full()).expect("saved");
    assert_eq!(store.load().expect("loaded"), Loaded::Present(full()));
    let mut next = full();
    next.under_way.remove(0);
    store.save(&next).expect("saved");
    assert_eq!(store.load().expect("loaded"), Loaded::Present(next));
    // A file beside it from a save that did not finish is not read.
    std::fs::write(store.dir().join(".record.tmp"), b"half").expect("written");
    assert!(matches!(store.load(), Ok(Loaded::Present(_))));
}

/// The marker's bytes are pinned, are written by the first save and
/// again by a save that finds them missing or other, and nothing else
/// counts as the marker (L4 of #149's review). Mutants: no marker
/// written; written only where the file is absent; any file of the name
/// read as the marker.
#[test]
fn the_first_save_writes_the_agent_s_marker() {
    assert_eq!(marker_bytes(), *b"SZAD\x01\x00");
    let (_root, store) = store();
    let marker = store.dir().join(MARKER);
    assert!(!store.marked().expect("read"));
    assert!(!marker.exists());
    store.save(&full()).expect("saved");
    assert!(store.marked().expect("read"));
    assert_eq!(std::fs::read(&marker).expect("read"), marker_bytes());
    std::fs::remove_file(&marker).expect("removed");
    assert!(!store.marked().expect("read"));
    store.save(&full()).expect("saved");
    assert!(store.marked().expect("read"));
    for other in [&b"SZAD\x02\x00"[..], b"SZAD", b"", b"SZAG\x01\x00"] {
        std::fs::write(&marker, other).expect("written");
        assert!(!store.marked().expect("read"), "{other:?}");
        store.save(&full()).expect("saved");
        assert_eq!(std::fs::read(&marker).expect("read"), marker_bytes());
    }
}

/// A corrupt record is reported and kept: neither a save nor a load
/// writes over it. Mutant: save writes over whatever is there.
#[test]
fn a_corrupt_record_is_not_written_over() {
    let (_root, store) = store();
    store.save(&full()).expect("saved");
    let path = store.dir().join(RECORD);
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[20] ^= 1;
    std::fs::write(&path, &bytes).expect("written");
    assert_eq!(
        store.load().expect("loaded"),
        Loaded::Corrupt(Corrupt::Hash)
    );
    let e = store.save(&Record::new([2; 16])).expect_err("not saved");
    assert!(matches!(e, SaveError::Corrupt(Corrupt::Hash)), "{e}");
    assert_eq!(std::fs::read(&path).expect("read"), bytes);
    // A record that breaks a rule is not written, whatever is there.
    let mut nine = full();
    nine.under_way = vec![nine.under_way[0].clone(); 9];
    std::fs::remove_file(&path).expect("removed");
    assert!(matches!(store.save(&nine), Err(SaveError::Rule(_))));
    assert_eq!(store.load().expect("loaded"), Loaded::Absent);
}

/// Mutant: a manifest not synced where it is read, not removed, or a
/// name that leaves the directory taken.
#[test]
fn manifests_are_kept_by_name_beside_the_record() {
    let (_root, store) = store();
    let a = "skz-20261125T010000Z";
    let b = "skz-20261125T010000Z-1";
    assert_eq!(store.read_manifest(a).expect("read"), None);
    assert_eq!(
        store.manifest_names().expect("listed"),
        Vec::<String>::new()
    );
    store.write_manifest(b, b"{}").expect("written");
    store.write_manifest(a, b"{\"a\":1}").expect("written");
    store
        .write_manifest(a, b"{\"a\":2}")
        .expect("written again");
    assert_eq!(
        store.read_manifest(a).expect("read"),
        Some(b"{\"a\":2}".to_vec())
    );
    assert_eq!(store.manifest_names().expect("listed"), [a, b]);
    store.remove_manifest(a).expect("removed");
    store.remove_manifest(a).expect("removed again");
    assert_eq!(store.read_manifest(a).expect("read"), None);
    assert_eq!(store.manifest_names().expect("listed"), [b]);
    for bad in ["..", ".", "../record", "a/b", ""] {
        let e = store.write_manifest(bad, b"x").expect_err(bad);
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad}");
        assert!(store.read_manifest(bad).is_err(), "{bad}");
        assert!(store.remove_manifest(bad).is_err(), "{bad}");
    }
    assert!(matches!(store.load(), Ok(Loaded::Absent)));
}

/// One publish in each phase and one name given up for each reason, each
/// field a value of its own: the record [`GOLDEN`] holds.
fn golden() -> Record {
    let taken = |group: u64, guid: u64, hash: u8| Taken {
        group,
        members: vec![Member {
            name: "p/d".into(),
            snapshot_guid: guid,
        }],
        manifest_len: 300,
        manifest_hash: [hash; 32],
    };
    let publish = |name: &str, phase: Phase, held: bool, attempts: u32| Publish {
        name: name.into(),
        count: Count {
            attachment: [0xa1; 16],
            state: [0x5e; 16],
            after: 3,
            load_guid: 0x10ad,
        },
        phase,
        held,
        attempts,
        stalls: 20 + attempts,
        begun_ms: 0x60 + u64::from(attempts),
        // Each value, so that the golden bytes pin every encoding.
        person: match attempts {
            2 => Some(Person::Stopped),
            3 => Some(Person::GiveUp),
            _ => None,
        },
    };
    let whys = [
        Why::ManifestLost,
        Why::ManifestTooLarge,
        Why::Destroyed,
        Why::Replaced,
        Why::Premise,
        Why::NameTaken,
        Why::Rewound,
        Why::NoProgress,
        Why::TooOld,
        Why::Stopped,
        Why::Person,
        Why::NotWhole,
    ];
    Record {
        volume: [0x0f; 16],
        enrolled: Some(Enrolled {
            pool: "p".into(),
            pool_guid: 0x81,
            leaf_guid: 0x82,
        }),
        under_way: vec![
            publish("i", Phase::Intent, false, 0),
            publish("t", Phase::Taken(taken(40, 1, 0x41)), false, 1),
            publish("h", Phase::Held(taken(41, 2, 0x42)), true, 2),
            publish(
                "a",
                Phase::Asked {
                    taken: taken(42, 3, 0x43),
                    asked_ms: 6,
                    held_until_ms: 7,
                },
                true,
                3,
            ),
            publish(
                "d",
                Phase::Done {
                    taken: taken(43, 4, 0x44),
                    commit: [0xc0; 32],
                },
                true,
                4,
            ),
        ],
        abandoned: whys
            .iter()
            .zip(0u64..)
            .map(|(&why, i)| Abandoned {
                name: format!("w{i}"),
                why,
                // Not the variant's index: `why` and `at_ms` must differ
                // for a swap of the two to change the bytes.
                at_ms: 1000 + i,
            })
            .collect(),
    }
}

/// The bytes of [`golden`] at version 2. postcard writes fields and
/// variants by position, so a field moved or added, or a variant inserted
/// before another, changes these bytes without a test of reading back
/// noticing: such a change is a new version ([`VERSION`]).
const GOLDEN: [&str; 18] = [
    "535a414702000000020200000f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f01017081",
    "018201050169a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e",
    "5e5e5e5e5e5e03ad210000001460000174a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad2101280103702f6401ac024141",
    "4141414141414141414141414141414141414141414141414141414141410001",
    "1561000168a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e",
    "5e5e5e5e5e03ad2102290103702f6402ac024242424242424242424242424242",
    "4242424242424242424242424242424242420102166201000161a1a1a1a1a1a1",
    "a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad21032a01",
    "03702f6403ac0243434343434343434343434343434343434343434343434343",
    "4343434343434306070103176301010164a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
    "a15e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e03ad21042b0103702f6404ac024444",
    "444444444444444444444444444444444444444444444444444444444444c0c0",
    "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c00104",
    "1864000c02773000e80702773101e90702773202ea0702773303eb0702773404",
    "ec0702773505ed0702773606ee0702773707ef0702773808f00702773909f107",
    "037731300af207037731310bf307de59616f3f9c3d57cc676fb03063590b5f74",
    "84c62c3cb819be7cea6883bc1446",
];

/// Mutant: two fields of a type swapped, a variant inserted before
/// another, or the head's order changed. Any of these is a record of
/// another version read as version 2.
#[test]
fn version_2s_bytes_are_these() {
    let hex: String = GOLDEN.concat();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
        .collect();
    assert_eq!(golden().encode().expect("encoded"), bytes);
    assert_eq!(Record::decode(&bytes), Ok(golden()));
}

/// Mutant: bytes left after the body read past.
#[test]
fn bytes_after_the_body_are_corrupt() {
    let record = golden();
    let mut body = postcard::to_allocvec(&record).expect("encoded");
    assert_eq!(Record::decode(&frame(&body)), Ok(record));
    body.push(0);
    assert_eq!(
        Record::decode(&frame(&body)),
        Err(Corrupt::Body("1 bytes after the body".into()))
    );
}

/// Mutant: a relative store path of one part (`agent`) has an empty
/// parent, and its entry in the current directory is never synced.
#[test]
fn the_directory_synced_for_a_new_one_is_the_one_that_holds_it() {
    assert_eq!(parent_of(Path::new("agent")), Some(Path::new(".")));
    assert_eq!(parent_of(Path::new("a/agent")), Some(Path::new("a")));
    assert_eq!(parent_of(Path::new("/var/agent")), Some(Path::new("/var")));
    assert_eq!(parent_of(Path::new("/agent")), Some(Path::new("/")));
    assert_eq!(parent_of(Path::new("/")), None);
}
