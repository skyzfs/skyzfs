//! Next look at #147, scratch tests of the adapter (planted in a
//! worktree at the head as `crates/skyzfs-agent/src/zfs/cli/nl147_cli.rs`
//! with `#[cfg(test)] mod nl147_cli;` in `cli.rs`, then removed).
//!
//! a1: the snapshot program's output, as `nl147_sandbox.lua` printed it
//!     (the program run in a sandbox of the channel programs' globals,
//!     its JSON as nvlist_print_json in the locale C), read by the
//!     adapter: every local value comes back as the bytes the program
//!     was given (`*.expected`, written by string.byte, not by the
//!     program's hex), and the manifest made of it encodes and decodes
//!     to the same.
//! a2: every byte value 0..255 alone, all 256 together, and the empty
//!     value: one form each in the manifest; decode(encode) the same.
//! a3: alternate forms refused by `Manifest::decode` (upper-case hex,
//!     hex of a printable value, a printable value as a hex object, the
//!     empty value in hex, a non-printable value as a string, odd
//!     length, a sign, non-ASCII digits).
//! a4: the cut output of the planted program (no hex) destroys
//!     `tank@t` and says so.
//! a5: hold: a partial hold (root held, child not; child held, root not)
//!     is an error and never Ok; a full one is Ok; the tag in the
//!     command line is never matched.
//! a6: `zpool get` says `missing pool name.` and `zpool status` hangs
//!     (killed: an error of the run): the original error, not NoPool.
//! a7: a drifted program (upper-case hex, 0x7f as a string, hex of a
//!     space) is refused and its take destroyed.
//! a8: the program at the head as ZFS ran it (recorder run 45665823917,
//!     ubuntu-26.04, records `*-snapshot-odd*`, in `nl147_zfs/`, the
//!     pool renamed `tank`): read by the adapter with no destroy, the
//!     values beyond ASCII come back as the bytes `zfs set` was given.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;
use crate::host::fake::{output, Fake};
use crate::manifest::{Manifest, ManifestError};

const WORDS: &str = "zfs program -j -t 100000000 -m 104857600 tank - tank t skyzfs:assist";
const DESTROY: &str = "zfs destroy -r tank@t";

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("NL147_LUA_OUT").expect("NL147_LUA_OUT"))
}

fn run_program(stdout: &[u8]) -> (Result<super::super::Program, ZfsError>, Vec<String>) {
    let mut fake = Fake::default();
    fake.give(
        WORDS,
        Ok(Output {
            code: Some(0),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }),
    );
    fake.ok(DESTROY, "");
    let mut zfs = ZfsCli::new(fake, Limits::default());
    let done = zfs.snapshot("tank", "t");
    let words = zfs.host().words().into_iter().map(String::from).collect();
    (done, words)
}

fn expected(name: &str) -> BTreeMap<(String, String), Vec<u8>> {
    let text = std::fs::read_to_string(dir().join(name)).expect("expected");
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 3 {
            continue;
        }
        let bytes = if fields[2].is_empty() {
            Vec::new()
        } else {
            fields[2]
                .split(',')
                .map(|b| b.parse::<u8>().expect("a byte"))
                .collect()
        };
        out.insert((fields[0].to_string(), fields[1].to_string()), bytes);
    }
    out
}

#[test]
fn a1_the_sandboxed_programs_output_reads_back_to_its_bytes() {
    for scenario in ["bytes", "wide", "wide_plain"] {
        let json = std::fs::read(dir().join(format!("{scenario}.json"))).expect("json");
        let (done, words) = run_program(&json);
        assert_eq!(words, [WORDS], "{scenario}: no destroy");
        let program = done.unwrap_or_else(|e| panic!("{scenario}: {e:?}"));
        let want = expected(&format!("{scenario}.expected"));
        let mut got = BTreeMap::new();
        for m in &program.members {
            for (k, v) in &m.local {
                got.insert((m.name.clone(), k.clone()), v.clone());
            }
        }
        assert_eq!(got.len(), want.len(), "{scenario}: count");
        for (key, bytes) in &want {
            assert_eq!(got.get(key), Some(bytes), "{scenario}: {key:?}");
        }
        let manifest = Manifest {
            pool: "tank".into(),
            pool_guid: 1,
            snapshot: "t".into(),
            createtxg: program.createtxg,
            members: program.members.clone(),
        };
        match manifest.encode() {
            Ok(bytes) => {
                let back = Manifest::decode(&bytes).expect("decoded");
                assert_eq!(back, manifest, "{scenario}");
                eprintln!("a1 {scenario}: {} members, manifest {} bytes", program.members.len(), bytes.len());
            }
            Err(e) => eprintln!("a1 {scenario}: manifest not encoded: {e}"),
        }
    }
}

fn one(value: Vec<u8>) -> Manifest {
    Manifest {
        pool: "tank".into(),
        pool_guid: 1,
        snapshot: "t".into(),
        createtxg: 2,
        members: vec![crate::manifest::Member {
            name: "tank".into(),
            kind: crate::manifest::Kind::Filesystem,
            guid: 3,
            snapshot_guid: 4,
            local: BTreeMap::from([("u:v".to_string(), value)]),
        }],
    }
}

fn with_local(value_json: &str) -> Vec<u8> {
    format!(
        r#"{{"createtxg":2,"format":"skyzfs-manifest","members":[{{"guid":"3","local":{{"u:v":{value_json}}},"name":"tank","snapshot_guid":"4","type":"filesystem"}}],"pool":"tank","pool_guid":"1","snapshot":"t","version":1}}"#
    )
    .into_bytes()
}

#[test]
fn a2_every_byte_value_has_one_form_and_round_trips() {
    let mut values: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
    values.push((0..=255u8).collect());
    values.push(Vec::new());
    values.push("café".as_bytes().to_vec());
    values.push(b"a\"b\\c".to_vec());
    for value in values {
        let m = one(value.clone());
        let bytes = m.encode().expect("encoded");
        assert_eq!(Manifest::decode(&bytes).expect("decoded"), m, "{value:?}");
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        let printable = value.iter().all(|b| (0x20..=0x7e).contains(b));
        assert_eq!(
            text.contains(r#"{"encoding":"hex""#),
            !printable,
            "{value:?}: {text}"
        );
        if !printable {
            let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(
                bytes,
                with_local(&format!(r#"{{"encoding":"hex","value":"{hex}"}}"#)),
                "{value:?}"
            );
        }
    }
}

#[test]
fn a3_alternate_forms_are_refused() {
    let cases = [
        (r#"{"encoding":"hex","value":"E9"}"#, "upper case"),
        (r#"{"encoding":"hex","value":"eA"}"#, "mixed case"),
        (r#"{"encoding":"hex","value":"61"}"#, "hex of a printable value"),
        (r#"{"encoding":"hex","value":""}"#, "the empty value in hex"),
        (r#""café""#, "a non-printable value as a string"),
        (r#""café""#, "UTF-8 as a string"),
        (r#""a\tb""#, "a tab as a string"),
        (r#"{"encoding":"hex","value":"e"}"#, "odd length"),
        (r#"{"encoding":"hex","value":"+e"}"#, "a sign"),
        (r#"{"encoding":"hex","value":"é9"}"#, "a non-ASCII digit"),
        (r#"{"encoding":"hex","value":"e9","x":1}"#, "a key more"),
        (r#"{"value":"e9","encoding":"hex"}"#, "keys out of order"),
        (r#"{"encoding":"HEX","value":"e9"}"#, "the encoding's case"),
    ];
    for (json, what) in cases {
        let e = Manifest::decode(&with_local(json));
        assert!(e.is_err(), "{what}: {e:?}");
        eprintln!("a3 {what}: {:?}", e.err());
    }
    // The canonical forms are read.
    assert!(Manifest::decode(&with_local(r#"{"encoding":"hex","value":"e9"}"#)).is_ok());
    assert!(Manifest::decode(&with_local(r#""a""#)).is_ok());
    assert!(Manifest::decode(&with_local(r#""""#)).is_ok());
    let _ = ManifestError::NotCanonical;
}

#[test]
fn a4_the_cut_output_of_a_program_without_hex_destroys_what_it_took() {
    let json = std::fs::read(dir().join("nohex-bytes.json")).expect("json");
    assert!(!json.ends_with(b"}\n"), "the plant's output is cut");
    let (done, words) = run_program(&json);
    assert_eq!(words, [WORDS, DESTROY]);
    match done {
        Err(ZfsError::Other(why)) => {
            assert!(why.contains("tank@t destroyed"), "{why}");
            eprintln!("a4: {why}");
        }
        other => panic!("{other:?}"),
    }
}

const HOLD: &str = "zfs hold -r skyzfs-publish tank@t";
const MEMBERS: &str = "zfs program -n -j -t 100000000 -m 104857600 tank - tank t";
const HOLDS_R: &str = "zfs holds -H -r tank@t";
const EXISTS: &str = "cannot hold snapshot 'tank@t': tag already exists on this dataset\n";
const THREE: &str = r#"{"return":{"members":{"tank":"11","tank/a":"22","tank/b":"33"}}}"#;

fn hold_with(stderr: &str, holds: &str) -> Result<(), ZfsError> {
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, stderr)
        .ok(MEMBERS, THREE)
        .ok(HOLDS_R, holds);
    ZfsCli::new(fake, Limits::default()).hold("tank", "t", "skyzfs-publish")
}

#[test]
fn a5_a_partial_hold_is_never_held() {
    let line = |name: &str, tag: &str| format!("{name}\t{tag}\tWed Nov 25 10:00 2026\n");
    let all = [line("tank@t", "skyzfs-publish"), line("tank/a@t", "skyzfs-publish"), line("tank/b@t", "skyzfs-publish")].concat();
    assert_eq!(hold_with(EXISTS, &all), Ok(()));
    // The root alone.
    let root = line("tank@t", "skyzfs-publish");
    let e = hold_with(EXISTS, &root);
    assert!(matches!(&e, Err(ZfsError::Other(w)) if w.contains("tank/a@t") && w.contains("tank/b@t")), "{e:?}");
    // A child alone (the root lacks it).
    let child = line("tank/b@t", "skyzfs-publish");
    let e = hold_with(EXISTS, &child);
    assert!(matches!(&e, Err(ZfsError::Other(w)) if w.contains("tank@t,") || w.contains("tank@t")), "{e:?}");
    // Every member holds another tag that contains this one, or this
    // one with a suffix: not the tag.
    let near = [line("tank@t", "skyzfs-publish2"), line("tank/a@t", "x-skyzfs-publish"), line("tank/b@t", "skyzfs-publish ")].concat();
    assert!(hold_with(EXISTS, &near).is_err());
    // No holds output at all.
    assert!(hold_with(EXISTS, "").is_err());
    // The words "tag already exists" in a dataset's name, before the last
    // ": ", with another message after it: not read as the tag.
    let named = "cannot hold snapshot 'tank/tag already exists@t': pool I/O is currently suspended\n";
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, named);
    let e = ZfsCli::new(fake, Limits::default()).hold("tank", "t", "skyzfs-publish");
    assert!(matches!(&e, Err(ZfsError::Other(w)) if w.contains("suspended")), "{e:?}");
    // The message on stdout, not stderr: not read.
    let mut fake = Fake::default();
    fake.give(HOLD, Ok(output(1, "tag already exists", "something else\n")));
    let mut zfs = ZfsCli::new(fake, Limits::default());
    let e = zfs.hold("tank", "t", "skyzfs-publish");
    assert!(matches!(&e, Err(ZfsError::Other(_))), "{e:?}");
    assert_eq!(zfs.host().words(), [HOLD]);
}

#[test]
fn a6_missing_pool_name_and_a_status_that_fails_is_not_nopool() {
    const GET: &str = "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint tank";
    let mut fake = Fake::default();
    fake.fails(GET, 2, "missing pool name.\nusage:\n\tget ...\n");
    fake.give("zpool status -P tank", Err(std::io::ErrorKind::TimedOut));
    let e = ZfsCli::new(fake, Limits::default()).props("tank");
    assert!(matches!(&e, Err(ZfsError::Other(w)) if w.contains("missing pool name")), "{e:?}");
    // And `zpool status` says the pool is there but faulted.
    let mut fake = Fake::default();
    fake.fails(GET, 2, "missing pool name.\n");
    fake.fails("zpool status -P tank", 1, "cannot open 'tank': I/O error\n");
    let e = ZfsCli::new(fake, Limits::default()).props("tank");
    assert!(matches!(&e, Err(ZfsError::Other(w)) if w.contains("missing pool name")), "{e:?}");
}

/// a7: the planted programs' outputs (hex in upper case; 0x7f let
/// through as printable; the space hexed): the adapter refuses each and
/// destroys `tank@t`, so a drift of the Lua fails safe after the take.
#[test]
fn a7_a_drifted_program_is_refused_and_its_take_destroyed() {
    for plant in ["upper", "del127", "space"] {
        let json = std::fs::read(dir().join(format!("{plant}-bytes.json"))).expect("json");
        let (done, words) = run_program(&json);
        assert_eq!(words, [WORDS, DESTROY], "{plant}");
        match done {
            Err(ZfsError::Other(why)) => eprintln!("a7 {plant}: {}", &why[..why.len().min(200)]),
            other => panic!("{plant}: {other:?}"),
        }
    }
}

#[test]
fn a8_the_programs_output_on_zfs_reads_back_to_the_bytes_set() {
    let zdir = PathBuf::from(std::env::var("NL147_ZFS").expect("NL147_ZFS"));
    let utf8 = "na\u{ef}ve, \u{65e5}\u{672c}, \u{2205}".as_bytes().to_vec();
    let raw = b"a\xff\xfeb\xc3".to_vec();
    for (record, pool) in [
        ("rig-snapshot-odd", "skzq"),
        ("rig-snapshot-odd-utf8", "skzq"),
        ("mirror-snapshot-odd", "skzqm"),
    ] {
        let text = std::fs::read_to_string(zdir.join(record).join("stdout")).expect("stdout");
        let text = text.replace(pool, "tank");
        let (done, words) = run_program(text.as_bytes());
        assert_eq!(words, [WORDS], "{record}: no destroy");
        let program = done.unwrap_or_else(|e| panic!("{record}: {e:?}"));
        let child = program
            .members
            .iter()
            .find(|m| m.name == "tank/child")
            .expect("tank/child");
        assert_eq!(child.local.get("skyzfs:utf8"), Some(&utf8), "{record}");
        assert_eq!(child.local.get("skyzfs:raw"), Some(&raw), "{record}");
        let manifest = Manifest {
            pool: "tank".into(),
            pool_guid: 1,
            snapshot: "t".into(),
            createtxg: program.createtxg,
            members: program.members.clone(),
        };
        let bytes = manifest.encode().expect("encoded");
        assert_eq!(Manifest::decode(&bytes).expect("decoded"), manifest, "{record}");
        eprintln!("a8 {record}: {} members, manifest {} bytes", program.members.len(), bytes.len());
    }
}
