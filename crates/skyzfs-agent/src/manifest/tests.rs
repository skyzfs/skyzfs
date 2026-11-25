use super::*;

fn member(name: &str, kind: Kind, guid: u64, local: &[(&str, &str)]) -> Member {
    Member {
        name: name.into(),
        kind,
        guid,
        snapshot_guid: guid + 1,
        local: local
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn small() -> Manifest {
    Manifest {
        pool: "tank".into(),
        pool_guid: 12_345_678_901_234_567_890,
        snapshot: "skz-20261125T010506Z".into(),
        createtxg: 4242,
        members: vec![
            member("tank", Kind::Filesystem, 7, &[]),
            member(
                "tank/db",
                Kind::Filesystem,
                9,
                &[("recordsize", "8192"), ("com.example:tier", "hot")],
            ),
            member(
                "tank/vol",
                Kind::Volume,
                u64::MAX - 1,
                &[("volsize", "1073741824")],
            ),
        ],
    }
}

/// Version 1's bytes. Mutant: a key renamed or out of order, white
/// space, a GUID written as a number, a member's fields swapped.
#[test]
fn a_manifest_is_version_1s_canonical_json() {
    let bytes = small().encode().expect("encoded");
    let wanted = concat!(
        r#"{"createtxg":4242,"format":"skyzfs-manifest","members":["#,
        r#"{"guid":"7","local":{},"name":"tank","snapshot_guid":"8","type":"filesystem"},"#,
        r#"{"guid":"9","local":{"com.example:tier":"hot","recordsize":"8192"},"#,
        r#""name":"tank/db","snapshot_guid":"10","type":"filesystem"},"#,
        r#"{"guid":"18446744073709551614","local":{"volsize":"1073741824"},"#,
        r#""name":"tank/vol","snapshot_guid":"18446744073709551615","type":"volume"}],"#,
        r#""pool":"tank","pool_guid":"12345678901234567890","#,
        r#""snapshot":"skz-20261125T010506Z","version":1}"#
    );
    assert_eq!(String::from_utf8_lossy(&bytes), wanted);
    assert_eq!(hash(&bytes), *blake3::hash(wanted.as_bytes()).as_bytes());
    // Any reader of JSON reads the same content.
    let value: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(value["members"][2]["snapshot_guid"], "18446744073709551615");
}

/// Each escape's bytes, the short ones where JSON has one. Mutant: an
/// escape written another way (`\u0008` for `\b` reads back the same,
/// so only the bytes show it), or a character escaped that is not.
#[test]
fn a_string_is_written_with_the_shortest_escapes() {
    let mut out = Vec::new();
    string(
        &mut out,
        "\u{8}\u{c}\n\r\t\"\\\u{0}\u{1}\u{1f} \u{7f}/é漢🦀",
    );
    let wanted = "\"\\b\\f\\n\\r\\t\\\"\\\\\\u0000\\u0001\\u001f \u{7f}/é漢🦀\"";
    assert_eq!(String::from_utf8_lossy(&out), wanted);
    let read: String = serde_json::from_slice(&out).expect("a JSON string");
    assert_eq!(read, "\u{8}\u{c}\n\r\t\"\\\u{0}\u{1}\u{1f} \u{7f}/é漢🦀");
}

/// Mutant: an escape left out or written another way than the reader's.
#[test]
fn every_manifest_reads_back_as_written() {
    let mut odd = small();
    odd.members[1].local = [
        ("q\"uote", "back\\slash"),
        ("lines", "a\nb\rc\td\u{8}e\u{c}f"),
        ("low", "\u{1}\u{1f}\u{7f}"),
        ("utf8", "é ü 漢 🦀"),
        ("empty", ""),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    odd.pool_guid = 0;
    odd.createtxg = u64::MAX;
    for manifest in [small(), odd] {
        let bytes = manifest.encode().expect("encoded");
        assert!(serde_json::from_slice::<Value>(&bytes).is_ok());
        assert_eq!(Manifest::decode(&bytes), Ok(manifest));
    }
}

/// Mutant: a reader that takes the content in another form, or another
/// format, version, key or type.
#[test]
fn only_the_canonical_bytes_of_version_1_are_read() {
    let good = String::from_utf8(small().encode().expect("encoded")).expect("UTF-8");
    let refused = |text: String| Manifest::decode(text.as_bytes());
    assert!(matches!(refused("{".into()), Err(ManifestError::Json(_))));
    assert!(matches!(refused("[]".into()), Err(ManifestError::Shape(_))));
    for (from, to, what) in [
        ("{\"createtxg\"", "{ \"createtxg\"", "white space"),
        (",\"version\":1}", ",\"version\":1}\n", "a line's end"),
        ("\"tank/db\"", "\"\\u0074ank/db\"", "another escape"),
        (
            "\"pool\":\"tank\"",
            "\"pool\":\"tank\",\"pool\":\"tank\"",
            "a key twice",
        ),
    ] {
        assert_eq!(
            refused(good.replacen(from, to, 1)),
            Err(ManifestError::NotCanonical),
            "{what}"
        );
    }
    assert_eq!(
        refused(good.replace("skyzfs-manifest", "skynbd-manifest")),
        Err(ManifestError::Format)
    );
    assert_eq!(
        refused(good.replace("\"version\":1", "\"version\":2")),
        Err(ManifestError::Version("2".into()))
    );
    assert_eq!(
        refused(good.replace("\"version\":1", "\"version\":\"1\"")),
        Err(ManifestError::Version("\"1\"".into()))
    );
    for (from, to, what) in [
        (
            "\"pool_guid\":\"12345678901234567890\"",
            "\"pool_guid\":12345678901234567890",
            "a GUID as a number",
        ),
        (
            "\"guid\":\"7\"",
            "\"guid\":\"07\"",
            "a GUID with a leading zero",
        ),
        ("\"guid\":\"7\"", "\"guid\":\"-7\"", "a GUID below zero"),
        (
            "\"guid\":\"7\"",
            "\"guid\":\"18446744073709551616\"",
            "a GUID over 64 bits",
        ),
        (
            "\"createtxg\":4242",
            "\"createtxg\":\"4242\"",
            "a group as a string",
        ),
        (
            "\"createtxg\":4242",
            "\"createtxg\":-1",
            "a group below zero",
        ),
        (
            "\"type\":\"volume\"",
            "\"type\":\"snapshot\"",
            "another type",
        ),
        (
            "\"volsize\":\"1073741824\"",
            "\"volsize\":1073741824",
            "a property as a number",
        ),
        ("\"local\":{}", "\"local\":[]", "local as a list"),
        (",\"version\":1}", ",\"version\":1,\"x\":0}", "a key more"),
        ("\"snapshot_guid\":\"8\",", "", "a key missing"),
        (
            "\"members\":[",
            "\"members\":{\"a\":",
            "members as an object",
        ),
    ] {
        assert!(
            matches!(
                refused(good.replacen(from, to, 1)),
                Err(ManifestError::Shape(_)) | Err(ManifestError::Json(_))
            ),
            "{what}: {:?}",
            refused(good.replacen(from, to, 1))
        );
    }
}

/// Mutant: a rule of the members not checked, at write or at read.
#[test]
fn the_members_are_the_root_then_its_descendants_in_order() {
    let with = |change: &dyn Fn(&mut Manifest)| {
        let mut manifest = small();
        change(&mut manifest);
        manifest
    };
    let cases: [(&str, Manifest); 10] = [
        ("none", with(&|m| m.members.clear())),
        ("the root not first", with(&|m| m.members.swap(0, 1))),
        ("descendants out of order", with(&|m| m.members.swap(1, 2))),
        (
            "a member twice",
            with(&|m| m.members[2] = m.members[1].clone()),
        ),
        (
            "the root twice",
            with(&|m| m.members[1].name = "tank".into()),
        ),
        (
            "another pool's",
            with(&|m| m.members[1].name = "tankx/db".into()),
        ),
        // In order after the others, so only the pool's prefix refuses
        // them: `x` and `0` sort after `/`.
        (
            "another pool's, last",
            with(&|m| m.members[2].name = "tankx/vol".into()),
        ),
        (
            "another pool's, a prefix of its name",
            with(&|m| m.members[2].name = "tank0".into()),
        ),
        (
            "a name ending at /",
            with(&|m| m.members[1].name = "tank/".into()),
        ),
        ("a bad snapshot name", with(&|m| m.snapshot = "a:b".into())),
    ];
    for (what, manifest) in cases {
        assert!(
            matches!(manifest.encode(), Err(ManifestError::Rule(_))),
            "{what}: encoded"
        );
    }
    // A reader refuses them too: the root not first, written by hand.
    let good = String::from_utf8(small().encode().expect("encoded")).expect("UTF-8");
    let swapped = good.replacen("\"name\":\"tank\"", "\"name\":\"tank/aa\"", 1);
    assert!(matches!(
        Manifest::decode(swapped.as_bytes()),
        Err(ManifestError::Rule(_))
    ));
}

/// Mutant: no limit, or one off by a byte.
#[test]
fn a_manifest_over_the_protocols_limit_is_refused() {
    let at = |len: usize| {
        let mut manifest = small();
        let base = manifest.encode().expect("encoded").len();
        // The root's local properties are empty: the pad is their only
        // key, with no comma.
        assert!(manifest.members[0].local.is_empty());
        let pad = "p".repeat(len - base - "\"pad\":\"\"".len());
        manifest.members[0].local.insert("pad".into(), pad);
        manifest
    };
    let max = skyzfs_control::MANIFEST_MAX;
    let most = at(max).encode().expect("the most is taken");
    assert_eq!(most.len(), max);
    assert_eq!(Manifest::decode(&most).map(|m| m.members.len()), Ok(3));
    assert_eq!(at(max + 1).encode(), Err(ManifestError::TooLarge(max + 1)));
    let mut over = most.clone();
    over.push(b' ');
    assert_eq!(
        Manifest::decode(&over),
        Err(ManifestError::TooLarge(max + 1))
    );
}

/// The record keeps each member's `@t` by name.
#[test]
fn the_snapshots_are_the_members_names_and_snapshot_guids() {
    assert_eq!(
        small().snapshots(),
        [
            record::Member {
                name: "tank".into(),
                snapshot_guid: 8
            },
            record::Member {
                name: "tank/db".into(),
                snapshot_guid: 10
            },
            record::Member {
                name: "tank/vol".into(),
                snapshot_guid: u64::MAX
            },
        ]
    );
}
