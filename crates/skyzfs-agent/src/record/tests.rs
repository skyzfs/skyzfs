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
            publish(
                "skz-20261125T010600Z",
                Phase::Abandoning {
                    why: Why::TooOld,
                    left: Left::Members(taken(45).members),
                },
                false,
            ),
        ],
        abandoned: vec![
            Abandoned {
                name: "skz-20261125T005900Z".into(),
                why: Why::ManifestLost,
                at_ms: 5,
                left: Left::Unknown,
            },
            Abandoned {
                name: "skz-20261125T005800Z".into(),
                why: Why::Destroyed,
                at_ms: 4,
                left: Left::Nothing,
            },
        ],
        published: vec![
            published("skz-20261125T005000Z", None),
            published("skz-20261125T005100Z", Some(Retire::Ref)),
            published("skz-20261125T005200Z", Some(Retire::Members)),
            published("skz-20261125T005300Z", Some(Retire::Other)),
        ],
        stuck: vec![Stuck {
            name: "skz-20261125T004900Z".into(),
            members: taken(46).members,
            at_ms: 9,
        }],
    }
}

/// A name published, with its retirement at `retire`.
pub(super) fn published(name: &str, retire: Option<Retire>) -> Published {
    Published {
        name: name.into(),
        commit: [0xc1; 32],
        group: 30,
        members: taken(30).members,
        at_ms: 1_700_000_100_000,
        retire,
    }
}

/// Mutant: a field dropped or swapped, in the body or the head.
#[test]
fn a_record_reads_back_as_written() {
    for record in [Record::new([1; 16]), full()] {
        let bytes = record.encode().expect("encoded");
        assert_eq!(&bytes[..4], b"SZAG");
        assert_eq!(&bytes[4..8], &[3, 0, 0, 0]);
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
            left: Left::Nothing,
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
fn a_head_that_is_not_version_3s_is_named() {
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
    assert_eq!(with(4, &4u16.to_le_bytes()), Err(Corrupt::Version(4)));
    // A body of version 3 under a head of version 2 is not read as one.
    assert!(matches!(
        with(4, &2u16.to_le_bytes()),
        Err(Corrupt::Body(_))
    ));
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
    kept.abandoned = vec![kept.abandoned[0].clone(); ABANDONED_MAX + 1];
    broken.push(("too many given up", kept));
    let mut no_members = full();
    no_members.abandoned[0].left = Left::Members(Vec::new());
    broken.push(("given up leaving no members", no_members));
    let mut held_giving_up = full();
    held_giving_up.under_way[6].held = true;
    broken.push(("held while given up", held_giving_up));
    let many = |n: usize, retire: Option<Retire>| -> Vec<Published> {
        (0..n)
            .map(|i| published(&format!("skz-20261101T{:06}Z", i), retire))
            .collect()
    };
    let mut kept_published = full();
    kept_published.published = many(PUBLISHED_MAX + 1, None);
    broken.push(("too many published", kept_published));
    let mut retiring = full();
    retiring.published = many(RETIRING_MAX + 1, Some(Retire::Ref));
    broken.push(("too many being retired", retiring));
    let mut published_twice = full();
    published_twice
        .published
        .push(published_twice.published[0].clone());
    broken.push(("published twice", published_twice));
    let mut published_under_way = full();
    let under_way_name = published_under_way.under_way[0].name.clone();
    published_under_way.published[0].name = under_way_name;
    broken.push(("published and under way", published_under_way));
    let mut published_empty = full();
    published_empty.published[0].members.clear();
    broken.push(("published with no members", published_empty));
    let mut published_badly = full();
    published_badly.published[0].name = "a/b".into();
    broken.push(("published with a bad name", published_badly));
    let stuck_many = |n: usize| -> Vec<Stuck> {
        (0..n)
            .map(|i| Stuck {
                name: format!("skz-20261001T{i:06}Z"),
                members: taken(46).members,
                at_ms: 9,
            })
            .collect()
    };
    let mut too_stuck = full();
    too_stuck.stuck = stuck_many(STUCK_MAX + 1);
    broken.push(("too many stuck", too_stuck));
    let mut stuck_twice = full();
    stuck_twice.stuck.push(stuck_twice.stuck[0].clone());
    broken.push(("stuck twice", stuck_twice));
    for other in ["under way", "given up", "published"] {
        let mut stuck_too = full();
        stuck_too.stuck[0].name = match other {
            "under way" => stuck_too.under_way[0].name.clone(),
            "given up" => stuck_too.abandoned[0].name.clone(),
            _ => stuck_too.published[0].name.clone(),
        };
        broken.push(("stuck and kept otherwise too", stuck_too));
    }
    let mut stuck_empty = full();
    stuck_empty.stuck[0].members.clear();
    broken.push(("stuck with no members", stuck_empty));
    let mut stuck_badly = full();
    stuck_badly.stuck[0].name = "a/b".into();
    broken.push(("stuck with a bad name", stuck_badly));
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
    // Eight under way, the most, the most given up, the most published
    // and the most of them being retired, are a record.
    let mut most = full();
    most.under_way = (0..UNDER_WAY_MAX)
        .map(|i| publish(&name(i), Phase::Intent, false))
        .collect();
    most.abandoned = vec![most.abandoned[0].clone(); ABANDONED_MAX];
    most.published = many(KEEP_MAX, None);
    most.published.extend(
        many(KEEP_MAX + RETIRING_MAX, Some(Retire::Members))
            .into_iter()
            .skip(KEEP_MAX),
    );
    most.stuck = stuck_many(STUCK_MAX);
    assert!(most.encode().is_ok());
}

/// A name given up leaves those under way and is kept with what it left;
/// none is dropped to make room (version 3: retention decides what
/// becomes of each). Mutant: a name dropped.
#[test]
fn a_name_given_up_leaves_those_under_way_and_is_kept() {
    let mut record = full();
    let name = record.under_way[3].name.clone();
    let left = Left::Members(taken(42).members);
    record.abandon(&name, Why::Destroyed, 77, left.clone());
    assert!(record.publish(&name).is_none());
    assert_eq!(record.under_way.len(), 6);
    assert_eq!(
        record.abandoned.last(),
        Some(&Abandoned {
            name,
            why: Why::Destroyed,
            at_ms: 77,
            left,
        })
    );
    for i in 0..ABANDONED_MAX as u64 {
        record.abandon(&format!("n{i}"), Why::Premise, 100 + i, Left::Nothing);
    }
    assert_eq!(record.abandoned.len(), ABANDONED_MAX + 3);
    assert_eq!(record.abandoned[0].at_ms, 5);
}

/// A publish done leaves those under way for the names published, with
/// its commit, its group and its members; one not done does not move.
/// Mutants: the commit, the group or the members not carried; a publish
/// not done moved.
#[test]
fn a_publish_done_is_kept_among_the_names_published() {
    let mut record = full();
    record.published.clear();
    record.leave_published("skz-20261125T010300Z", 9);
    assert!(record.published.is_empty(), "not done");
    record.leave_published("skz-20261125T010500Z", 9);
    assert!(record.publish("skz-20261125T010500Z").is_none());
    assert_eq!(
        record.published,
        vec![Published {
            name: "skz-20261125T010500Z".into(),
            commit: [0xc0; 32],
            group: 44,
            members: taken(44).members,
            at_ms: 9,
            retire: None,
        }]
    );
    assert!(record.names("skz-20261125T010500Z"));
    assert!(record.names("skz-20261125T005900Z"));
    assert!(record.names("skz-20261125T010000Z"));
    assert!(!record.names("skz-20261125T020000Z"));
}

/// A publish is begun only where it, and each under way, would find room
/// whichever way it ends. Mutants: the room of the names given up or of
/// those published not asked; the one to begin not counted.
#[test]
fn a_publish_is_begun_only_with_room_for_its_end() {
    let mut record = full();
    record.under_way.truncate(2);
    assert!(record.room());
    let given_up = record.abandoned[0].clone();
    record.abandoned = vec![given_up.clone(); ABANDONED_MAX - 3];
    assert!(record.room());
    record.abandoned.push(given_up);
    assert!(!record.room(), "two under way and one more: no room");
    record.abandoned.truncate(ABANDONED_MAX - 3);
    let one = record.published[0].clone();
    record.published = vec![one.clone(); PUBLISHED_MAX - 3];
    assert!(record.room());
    record.published.push(one);
    assert!(!record.room());
    record.published.truncate(1);
    record.under_way = (0..UNDER_WAY_MAX)
        .map(|i| publish(&format!("skz-20261125T0200{i:02}Z"), Phase::Intent, false))
        .collect();
    assert!(!record.room(), "eight under way");
}

/// At `--keep` at its bound, with as many names being retired as there
/// may be and none of them leaving (stopped `other`, or their retires
/// unanswered), the record still has room for [`PUBLISHED_SLACK`] (64)
/// publishes (the lead's ruling on decision 5 of #179's review).
/// Mutant: no slack.
#[test]
fn at_the_bound_of_keep_a_slack_of_publishes_is_left() {
    let mut record = full();
    record.under_way.clear();
    let one = record.published[0].clone();
    let held = KEEP_MAX + RETIRING_MAX;
    record.published = vec![one.clone(); held];
    assert!(record.room());
    record.published = vec![one.clone(); held + PUBLISHED_SLACK - 1];
    assert!(record.room(), "the slack's last");
    record.published.push(one);
    assert!(!record.room());
    assert!(PUBLISHED_SLACK >= 64);
}

/// Where the names given up alone stand between the record and its
/// room, [`Record::abandoned_over`] counts them; where the publishes
/// under way or the names published are short of it too, none (the
/// lead's ruling on M1 of #179's review). Mutants: counted whatever
/// else is short, one off.
#[test]
fn the_names_given_up_over_the_room_are_counted_where_alone() {
    let mut record = full();
    record.under_way.truncate(2);
    let given_up = record.abandoned[0].clone();
    record.abandoned = vec![given_up.clone(); ABANDONED_MAX - 3];
    record.published.truncate(1);
    assert_eq!(record.abandoned_over(), 0);
    record.abandoned.push(given_up.clone());
    assert_eq!(record.abandoned_over(), 1);
    record.abandoned.push(given_up);
    assert_eq!(record.abandoned_over(), 2);
    let one = record.published[0].clone();
    record.published = vec![one; PUBLISHED_MAX];
    assert_eq!(record.abandoned_over(), 0, "the published are short too");
    record.published.truncate(1);
    record.under_way = (0..UNDER_WAY_MAX)
        .map(|i| publish(&format!("skz-20261125T0200{i:02}Z"), Phase::Intent, false))
        .collect();
    assert_eq!(record.abandoned_over(), 0, "eight under way");
}

/// The names stuck count for no room: the most of them, beside the
/// most published that leave room for one more, still let a publish
/// begin. Mutant: the names stuck counted among those published.
#[test]
fn the_names_stuck_count_for_no_room() {
    let mut record = full();
    record.under_way.truncate(2);
    let one = record.published[0].clone();
    record.published = vec![one; PUBLISHED_MAX - 3];
    let stuck = record.stuck[0].clone();
    record.stuck = (0..STUCK_MAX)
        .map(|i| Stuck {
            name: format!("skz-20261001T{i:06}Z"),
            ..stuck.clone()
        })
        .collect();
    assert!(record.room());
    assert!(record.names("skz-20261001T000063Z"));
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

/// One publish in each phase of version 2 and one name given up for each
/// reason, each field a value of its own: the record [`GOLDEN_V2`] holds,
/// as version 3 reads it (each name given up leaving [`Left::Unknown`]).
fn golden_v2() -> Record {
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
                left: Left::Unknown,
            })
            .collect(),
        published: Vec::new(),
        stuck: Vec::new(),
    }
}

/// [`golden_v2`] with what version 3 adds, each value of its own: a
/// publish being given up, each thing a give-up leaves, and a name
/// published at each step of its retirement, and a name stuck. The
/// record [`GOLDEN`] holds.
fn golden() -> Record {
    let mut record = golden_v2();
    let members = |guid: u64| {
        vec![Member {
            name: "p/e".into(),
            snapshot_guid: guid,
        }]
    };
    record.under_way.push(Publish {
        name: "g".into(),
        phase: Phase::Abandoning {
            why: Why::Rewound,
            left: Left::Members(members(5)),
        },
        held: false,
        ..record.under_way[0].clone()
    });
    record.abandoned[0].left = Left::Nothing;
    record.abandoned[1].left = Left::Members(members(6));
    let published = |name: &str, i: u8, retire: Option<Retire>| Published {
        name: name.into(),
        commit: [0xd0 + i; 32],
        group: 50 + u64::from(i),
        members: members(10 + u64::from(i)),
        at_ms: 2000 + u64::from(i),
        retire,
    };
    record.published = vec![
        published("p0", 0, None),
        published("p1", 1, Some(Retire::Ref)),
        published("p2", 2, Some(Retire::Members)),
        published("p3", 3, Some(Retire::Other)),
    ];
    record.stuck = vec![Stuck {
        name: "s0".into(),
        members: members(20),
        at_ms: 3000,
    }];
    record
}

/// The bytes of [`golden_v2`] as an agent of version 2 wrote them.
const GOLDEN_V2: [&str; 18] = [
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

/// The bytes of [`golden`] at version 3. postcard writes fields and
/// variants by position, so a field moved or added, or a variant inserted
/// before another, changes these bytes without a test of reading back
/// noticing: such a change is a new version ([`VERSION`]).
const GOLDEN: [&str; 26] = [
    "535a4147030000000b0300000f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f01017081",
    "018201060169a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e",
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
    "1864000167a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a15e5e5e5e5e5e5e5e5e5e5e",
    "5e5e5e5e5e03ad210506010103702f650500001460000c02773000e807000277",
    "3101e907010103702f650602773202ea070202773303eb070202773404ec0702",
    "02773505ed070202773606ee070202773707ef070202773808f0070202773909",
    "f10702037731300af20702037731310bf3070204027030d0d0d0d0d0d0d0d0d0",
    "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0320103702f650ad00f",
    "00027031d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1",
    "d1d1d1d1330103702f650bd10f0100027032d2d2d2d2d2d2d2d2d2d2d2d2d2d2",
    "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2340103702f650cd20f0101027033",
    "d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3",
    "350103702f650dd30f0102010273300103702f6514b8177ca47de4d9bdff3ed8",
    "81d9a1c4aa456ccb02adf889aa4a5b9eaa3750409df8f6",
];

fn unhex(lines: &[&str]) -> Vec<u8> {
    let hex: String = lines.concat();
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"))
        .collect()
}

/// Mutant: two fields of a type swapped, a variant inserted before
/// another, or the head's order changed. Any of these is a record of
/// another version read as version 3.
#[test]
fn version_3s_bytes_are_these() {
    let bytes = unhex(&GOLDEN);
    let encoded = golden().encode().expect("encoded");
    let hex: String = encoded.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(encoded, bytes, "{hex}");
    assert_eq!(Record::decode(&bytes), Ok(golden()));
}

/// A record of version 2 reads as version 3: nothing published, and each
/// name given up leaving [`Left::Unknown`]. Its next save is version 3.
/// Mutants: version 2 refused; its body read as version 3's; a name given
/// up read as leaving nothing.
#[test]
fn version_2s_bytes_read_as_version_3() {
    let bytes = unhex(&GOLDEN_V2);
    assert_eq!(Record::decode(&bytes), Ok(golden_v2()));
    let saved = golden_v2().encode().expect("encoded");
    assert_eq!(&saved[4..6], &VERSION.to_le_bytes());
    assert_ne!(saved, bytes);
    // A body of version 2 under a head of version 3 is not read.
    let mut as_3 = bytes.clone();
    as_3[4..6].copy_from_slice(&VERSION.to_le_bytes());
    let end = as_3.len() - SUM;
    let sum = blake3::hash(&as_3[..end]);
    as_3[end..].copy_from_slice(sum.as_bytes());
    assert!(Record::decode(&as_3).is_err());
    // Nor is a phase of version 3 under a head of version 2.
    let mut record = golden_v2();
    record.under_way[0].phase = Phase::Abandoning {
        why: Why::Person,
        left: Left::Nothing,
    };
    let v2 = postcard::to_allocvec(&(
        record.volume,
        &record.enrolled,
        &record.under_way,
        record
            .abandoned
            .iter()
            .map(|a| (&a.name, a.why, a.at_ms))
            .collect::<Vec<_>>(),
    ))
    .expect("a body");
    let mut framed = frame(&v2);
    framed[4..6].copy_from_slice(&VERSION_2.to_le_bytes());
    let end = framed.len() - SUM;
    let sum = blake3::hash(&framed[..end]);
    framed[end..].copy_from_slice(sum.as_bytes());
    assert_eq!(
        Record::decode(&framed),
        Err(Corrupt::Body(
            "a phase of version 3 in a record of version 2".into()
        ))
    );
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
