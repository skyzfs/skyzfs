//! The adapter on output recorded from ZFS on the rigs' images (the
//! plan's Q10): ZFS 2.2 userland over a 2.4.1 module on Ubuntu 24.04,
//! and 2.4.1 on 26.04. `recorded/<image>.json` holds, by name, each
//! command the recorder ran (`cmd`), its exit code and its two outputs;
//! its `source` names the recorder, the run and the artifact.
//!
//! Each test gives the fake host a record's output under the words the
//! adapter runs, checks that those words are the record's (the recorder
//! writes `\,` for its shell and feeds a program with `< file.lua`), and
//! checks what the adapter made of the output. Two pools were recorded:
//! `skzq`, the rig's (one leaf on `/dev/ublkb0`, a log on a loop
//! device), and `skzqm`, a mirror of two files. Both have the members
//! `child` and `vol`; the hold's tag is the agent's, `skyzfs-publish`.
//!
//! `recorded/full-<image>.json` holds, in the same shape, the count of
//! the datasets and the members program in full, recorded later
//! (testing's runs 45720134548 and 45720225770, on #153's `65d72ee`):
//! the rest of those runs is not taken, since the tests above read the
//! pools of the first runs by their GUIDs. The members program in full
//! is no longer run: the manifest is read by `zfs list` and `zfs get`
//! (M2.6c-5), whose recordings are to come; its records are kept as
//! they were recorded and not read.
//!
//! `recorded/read-<image>.json` holds the manifest's read by `zfs list`
//! and `zfs get` (M2.6c-5), recorded by testing's run 45738224324 (#156):
//! each pool's list, its values pass, and each odd value read alone. The
//! pairs pass (`-o name,property`) was not recorded: the test makes it
//! from the values pass ([`pairs_of`]). The outputs that are not UTF-8
//! (each pool's values pass and its `raw` value read alone) are kept as
//! their exact bytes (`stdout_hex`), taken again from the same artifact
//! (N7 of #157's review).
//!
//! `recorded/cli-read-<image>.json` holds testing's second recording of
//! the three reads (run 45751625028, #158) on one pool in four states,
//! with the pairs pass, a pool that does not exist, and output that is
//! not UTF-8 kept exact (`stdout_hex`). Its test also checks that the
//! pairs [`pairs_of`] makes are the recorded pairs.
//!
//! A `zpool status -P -g` was recorded beside some of the `-P` outputs
//! only. A `-P` output without its own is read with the `-g` output of
//! the same tree recorded nearest to it (`GUIDED`); only the shape of
//! the tree and its GUIDs are taken from it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::host::fake::{output, Fake};
use crate::zfs::{premise, HOLD_TAG};

/// The recordings, by image.
const IMAGES: [(&str, &str); 2] = [
    ("ubuntu-24.04", include_str!("recorded/ubuntu-24.04.json")),
    ("ubuntu-26.04", include_str!("recorded/ubuntu-26.04.json")),
];

/// The recordings of the count and of the members program in full, by
/// image.
const FULL_IMAGES: [(&str, &str); 2] = [
    (
        "ubuntu-24.04",
        include_str!("recorded/full-ubuntu-24.04.json"),
    ),
    (
        "ubuntu-26.04",
        include_str!("recorded/full-ubuntu-26.04.json"),
    ),
];

/// The recordings of the manifest's read by `zfs list` and `zfs get`
/// (testing's run 45738224324, #156), by image: each pool's list, its
/// values pass and each odd value read alone.
const READ_IMAGES: [(&str, &str); 2] = [
    (
        "ubuntu-24.04",
        include_str!("recorded/read-ubuntu-24.04.json"),
    ),
    (
        "ubuntu-26.04",
        include_str!("recorded/read-ubuntu-26.04.json"),
    ),
];

/// The recordings of the manifest's three reads on the pool `skzrd`
/// (testing's run 45751625028, #158), by image: each in the states
/// `tree`, `not` (`@t` destroyed), `odd` (one file system per odd value)
/// and `counts` (a file system and a snapshot limit set on `child`),
/// each odd value read alone, and the reads of a pool that does not
/// exist (`none`).
const CLI_READ_IMAGES: [(&str, &str); 2] = [
    (
        "ubuntu-24.04",
        include_str!("recorded/cli-read-ubuntu-24.04.json"),
    ),
    (
        "ubuntu-26.04",
        include_str!("recorded/cli-read-ubuntu-26.04.json"),
    ),
];

/// The pool of [`CLI_READ_IMAGES`].
const READ_POOL: &str = "skzrd";

/// The odd values the recorder set as `skyzfs:v`, by the name of their
/// dataset after `odd-`. `len8192` is too long: its set was refused.
const ODD: [&str; 14] = [
    "bslash", "cr", "dash", "empty", "forge", "len4095", "len4096", "len8191", "len8192", "nl",
    "raw", "spaces", "tab", "utf8",
];

/// The rig's pool and the mirror's.
const RIG: &str = "skzq";
const MIRROR: &str = "skzqm";
/// The pool that does not exist.
const NONE: &str = "skznone";

/// What the adapter asks where a command says its pool is not there.
const POOLS: &str = "zpool list -H -o name";
/// The tag the recorder held with: the agent's.
const TAG: &str = HOLD_TAG;
/// The device number the fake gives the rig's leaf.
const DEVICE: u64 = 0x0103_0000;

/// One image's records.
struct Recording {
    image: &'static str,
    records: serde_json::Map<String, Value>,
}

/// One command as recorded.
#[derive(Clone)]
struct Record {
    cmd: String,
    rc: i32,
    /// Standard output, its bytes that are not UTF-8 replaced.
    stdout: String,
    stderr: String,
    /// Standard output exactly: a record keeps output that is not UTF-8
    /// as `stdout_hex`.
    stdout_bytes: Vec<u8>,
}

fn recordings() -> Vec<Recording> {
    parse(&IMAGES)
}

fn full_recordings() -> Vec<Recording> {
    parse(&FULL_IMAGES)
}

fn parse(images: &[(&'static str, &'static str)]) -> Vec<Recording> {
    images
        .iter()
        .map(|(image, text)| {
            let value: Value = serde_json::from_str(text).expect("a recording is JSON");
            let records = value
                .get("records")
                .and_then(Value::as_object)
                .expect("a recording has `records`")
                .clone();
            Recording { image, records }
        })
        .collect()
}

impl Recording {
    fn get(&self, name: &str) -> Record {
        let record = self
            .records
            .get(name)
            .unwrap_or_else(|| panic!("{}: no record `{name}`", self.image));
        let bytes = |key: &str| -> Vec<u8> {
            if let Some(text) = record.get(key).and_then(Value::as_str) {
                return text.as_bytes().to_vec();
            }
            let hex = record
                .get(format!("{key}_hex"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{}: `{name}` has no `{key}`", self.image));
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
                .collect()
        };
        let text = |key: &str| String::from_utf8_lossy(&bytes(key)).into_owned();
        let rc = record
            .get("rc")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("{}: `{name}` has no `rc`", self.image));
        Record {
            cmd: text("cmd"),
            rc: i32::try_from(rc).expect("an exit code"),
            stdout: text("stdout"),
            stderr: text("stderr"),
            stdout_bytes: bytes("stdout"),
        }
    }

    /// The adapter on a fake host that gives `names`' outputs, in order,
    /// each under its record's words; the rig's leaf a block device and
    /// the mirror's files regular files.
    fn adapter(&self, names: &[&str]) -> ZfsCli<Fake> {
        let mut fake = Fake::default();
        for name in names {
            let record = self.get(name);
            fake.give(
                &record.words(),
                Ok(output(record.rc, &record.stdout, &record.stderr)),
            );
        }
        fake.nodes
            .insert(PathBuf::from("/dev/ublkb0"), Node::Block(DEVICE));
        fake.nodes
            .insert(PathBuf::from("/dev/ublkb1"), Node::Block(DEVICE + 1));
        for file in ["m0", "m1"] {
            fake.nodes.insert(
                PathBuf::from(format!("/mnt/skz-cli-record/{file}.img")),
                Node::File,
            );
        }
        fake.files.insert(
            PathBuf::from(NOCACHEFLUSH),
            self.get("nocacheflush").stdout.into_bytes(),
        );
        // Not recorded: the recorder's two pools, as `zpool list` would
        // list them where a command says its pool is not there.
        fake.ok(POOLS, &format!("{RIG}\n{MIRROR}\n"));
        ZfsCli::new(fake, Limits::default())
    }

    /// The words of `names`' records.
    fn words(&self, names: &[&str]) -> Vec<String> {
        names.iter().map(|name| self.get(name).words()).collect()
    }
}

impl Record {
    /// The command's words as the adapter runs them. A record made under
    /// `env LC_ALL=C.UTF-8` is given under the words without it: the
    /// adapter runs each command in the locale `C`, and the record shows
    /// what the other locale would change.
    fn words(&self) -> String {
        let line = self.cmd.trim_end().replace("\\,", ",");
        let line = line
            .strip_prefix("env LC_ALL=C.UTF-8 ")
            .map(str::to_string)
            .unwrap_or(line);
        match line.split_once(" < ") {
            Some((words, _program)) => words.to_string(),
            None => line,
        }
    }
}

/// The words the adapter ran.
fn ran(zfs: &mut ZfsCli<Fake>) -> Vec<String> {
    zfs.host().words().into_iter().map(String::from).collect()
}

fn disk(guid: u64, rdev: u64) -> Vdev {
    Vdev {
        kind: VdevKind::Disk,
        guid,
        rdev: Some(rdev),
        children: Vec::new(),
    }
}

fn file(guid: u64) -> Vdev {
    Vdev {
        kind: VdevKind::File,
        guid,
        rdev: None,
        children: Vec::new(),
    }
}

fn mirror(guid: u64, children: Vec<Vdev>) -> Vdev {
    Vdev {
        kind: VdevKind::Mirror,
        guid,
        rdev: None,
        children,
    }
}

/// The GUIDs of each image's vdevs, from its `-g` outputs: the rig's
/// leaf and, after the attach, its mirror and the new leaf; the mirror
/// pool's mirror and its two files.
struct Guids {
    leaf: u64,
    rig_mirror: u64,
    attached: u64,
    mirror: u64,
    m0: u64,
    m1: u64,
}

fn guids(image: &str) -> Guids {
    match image {
        "ubuntu-24.04" => Guids {
            leaf: 8930052142508163984,
            rig_mirror: 13916280981439498084,
            attached: 12714833013087552368,
            mirror: 9298558496200761961,
            m0: 13066833937729343271,
            m1: 962681601743306748,
        },
        "ubuntu-26.04" => Guids {
            leaf: 17837908470104127412,
            rig_mirror: 4990637876201622794,
            attached: 13062360064792703612,
            mirror: 9883761927635724529,
            m0: 536622382384900784,
            m1: 4958484949203869743,
        },
        other => panic!("no GUIDs for {other}"),
    }
}

#[test]
fn the_recordings_are_of_the_versions_the_docs_name() {
    for (rec, versions) in recordings().iter().zip([
        ["zfs-2.2.2-", "zfs-kmod-2.4.1-"],
        ["zfs-2.4.1-", "zfs-kmod-2.4.1-"],
    ]) {
        let version = rec.get("version").stdout;
        let lines: Vec<&str> = version.lines().collect();
        assert_eq!(lines.len(), 2, "{}: {version}", rec.image);
        for (line, prefix) in lines.iter().zip(versions) {
            assert!(line.starts_with(prefix), "{}: {version}", rec.image);
        }
    }
}

#[test]
fn the_rigs_pool_as_recorded_passes_the_premise() {
    for rec in recordings() {
        let g = guids(rec.image);
        let names = ["rig-status", "rig-status-g", "rig-get"];
        let mut zfs = rec.adapter(&names);
        let layout = zfs.layout(RIG).expect("the rig's layout");
        assert_eq!(
            layout,
            Layout {
                data: vec![disk(g.leaf, DEVICE)],
                special: 0,
                dedup: 0,
                other: 1,
                busy: None,
                removed: None,
            },
            "{}",
            rec.image
        );
        let props = zfs.props(RIG).expect("the rig's properties");
        let nocacheflush = zfs.nocacheflush().expect("the parameter");
        assert!(!nocacheflush, "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&names), "{}", rec.image);
        assert_eq!(
            premise(&layout, &props, nocacheflush, DEVICE, RIG, None),
            Ok(g.leaf),
            "{}",
            rec.image
        );
    }
}

#[test]
fn the_properties_read_as_recorded() {
    let expected = [
        // image, record, guid, load_guid, checkpoint
        (
            "ubuntu-24.04",
            "rig-get",
            7999830091773344557,
            14005298585444973037,
            false,
        ),
        (
            "ubuntu-24.04",
            "rig-get-checkpoint",
            7999830091773344557,
            14005298585444973037,
            true,
        ),
        (
            "ubuntu-24.04",
            "mirror-get",
            503252197325000815,
            11886059228267080567,
            false,
        ),
        (
            "ubuntu-24.04",
            "mirror-get-checkpoint",
            503252197325000815,
            11886059228267080567,
            true,
        ),
        (
            "ubuntu-26.04",
            "rig-get",
            17263484139300742590,
            10064081898206912642,
            false,
        ),
        (
            "ubuntu-26.04",
            "rig-get-checkpoint",
            17263484139300742590,
            10064081898206912642,
            true,
        ),
        (
            "ubuntu-26.04",
            "mirror-get",
            8544523855526204880,
            6971777658390579765,
            false,
        ),
        (
            "ubuntu-26.04",
            "mirror-get-checkpoint",
            8544523855526204880,
            6971777658390579765,
            true,
        ),
    ];
    let recordings = recordings();
    for (image, name, guid, load_guid, checkpoint) in expected {
        let rec = recordings
            .iter()
            .find(|r| r.image == image)
            .expect("the image");
        let pool = if name.starts_with("rig") { RIG } else { MIRROR };
        let mut zfs = rec.adapter(&[name]);
        assert_eq!(
            zfs.props(pool),
            Ok(Props {
                guid,
                load_guid,
                multihost: false,
                checkpoint,
                readonly: false,
            }),
            "{image} {name}"
        );
        assert_eq!(ran(&mut zfs), rec.words(&[name]), "{image} {name}");
    }
}

#[test]
fn a_checkpoint_as_recorded_is_refused() {
    for rec in recordings() {
        let names = ["rig-status", "rig-status-g", "rig-get-checkpoint"];
        let mut zfs = rec.adapter(&names);
        let layout = zfs.layout(RIG).expect("the rig's layout");
        let props = zfs.props(RIG).expect("the rig's properties");
        assert_eq!(
            premise(&layout, &props, false, DEVICE, RIG, None),
            Err(crate::zfs::Refusal::Checkpoint),
            "{}",
            rec.image
        );
    }
}

/// `zpool status -P` records, with the `-g` record each is read with
/// (module docs), and whether a resilver was under way when it was
/// recorded, by image (24.04, 26.04).
const GUIDED: [(&str, &str, [bool; 2]); 10] = [
    ("rig-status", "rig-status-g", [false, false]),
    ("rig-scrub-status", "rig-scrub-status-g", [false, false]),
    ("rig-scrub-paused", "rig-status-g", [false, false]),
    ("rig-scrub-canceled", "rig-status-g", [false, false]),
    ("rig-resilver-status", "rig-resilver-status-g", [true, true]),
    ("rig-resilvered", "rig-resilver-status-g", [false, false]),
    ("mirror-status", "mirror-status-g", [false, false]),
    (
        "mirror-scrub-status",
        "mirror-scrub-status-g",
        [false, false],
    ),
    (
        "mirror-resilver-status",
        "mirror-resilver-status-g",
        [true, true],
    ),
    (
        "mirror-resilvered",
        "mirror-resilver-status-g",
        [false, false],
    ),
];

#[test]
fn a_resilver_is_busy_and_a_scrub_is_not_as_recorded() {
    for (i, rec) in recordings().iter().enumerate() {
        let g = guids(rec.image);
        let rig_mirror = mirror(
            g.rig_mirror,
            vec![disk(g.leaf, DEVICE), disk(g.attached, DEVICE + 1)],
        );
        for (named, guided, busy) in GUIDED {
            let mut zfs = rec.adapter(&[named, guided]);
            let layout = zfs
                .layout(if named.starts_with("rig") {
                    RIG
                } else {
                    MIRROR
                })
                .unwrap_or_else(|e| panic!("{} {named}: {e:?}", rec.image));
            assert_eq!(ran(&mut zfs), rec.words(&[named, guided]));
            let data = match named {
                "rig-status" | "rig-scrub-status" | "rig-scrub-paused" | "rig-scrub-canceled" => {
                    vec![disk(g.leaf, DEVICE)]
                }
                "rig-resilver-status" | "rig-resilvered" => vec![rig_mirror.clone()],
                _ => vec![mirror(g.mirror, vec![file(g.m0), file(g.m1)])],
            };
            assert_eq!(layout.data, data, "{} {named}", rec.image);
            // The rig's pool has a log; the mirror's a special and a
            // dedup mirror, a cache and a spare.
            let classes = if named.starts_with("rig") {
                (0, 0, 1)
            } else {
                (1, 1, 2)
            };
            assert_eq!(
                (layout.special, layout.dedup, layout.other),
                classes,
                "{} {named}",
                rec.image
            );
            match &layout.busy {
                Some(what) => {
                    assert!(busy[i], "{} {named}: busy: {what}", rec.image);
                    assert!(
                        what.starts_with("scan: resilver in progress since "),
                        "{} {named}: {what}",
                        rec.image
                    );
                }
                None => assert!(!busy[i], "{} {named}: not busy", rec.image),
            }
        }
    }
}

#[test]
fn a_pool_that_does_not_exist_reads_as_recorded() {
    for rec in recordings() {
        // Each says no pool only once `zpool list` lists none it names.
        let gone = |name: &str| ZfsError::NoPool(rec.get(name).words());
        let confirmed = |names: &[&str]| {
            let mut words = rec.words(names);
            words.push(POOLS.to_string());
            words
        };
        let mut zfs = rec.adapter(&["none-status"]);
        assert_eq!(zfs.layout(NONE), Err(gone("none-status")), "{}", rec.image);
        assert_eq!(ran(&mut zfs), confirmed(&["none-status"]));

        // ZFS 2.2's `zpool get` says `missing pool name.`: the adapter
        // asks `zpool status`, as recorded beside it.
        let get = if rec.image == "ubuntu-24.04" {
            vec!["none-get", "none-status"]
        } else {
            vec!["none-get"]
        };
        let mut zfs = rec.adapter(&get);
        let last = get[get.len() - 1];
        assert_eq!(zfs.props(NONE), Err(gone(last)), "{}", rec.image);
        assert_eq!(ran(&mut zfs), confirmed(&get), "{}", rec.image);

        // `zfs snapshot -r` of a pool that does not exist says `dataset
        // does not exist` with its usage (exit 2): an error, neither a
        // take nor `EEXIST`. The engine's next round reads no pool.
        let mut zfs = rec.adapter(&["none-snapshot-cli"]);
        match zfs.snapshot(NONE, "t") {
            Err(ZfsError::Other(why)) => assert!(
                why.contains("zfs snapshot -r skznone@t: exit 2: cannot open 'skznone': dataset does not exist"),
                "{}: {why}",
                rec.image
            ),
            other => panic!("{}: {other:?}", rec.image),
        }
        assert_eq!(ran(&mut zfs), rec.words(&["none-snapshot-cli"]));
        // `zfs destroy -r` says `dataset does not exist`: nothing to
        // destroy.
        let mut zfs = rec.adapter(&["none-destroy"]);
        assert_eq!(zfs.destroy(NONE, "t"), Ok(()), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["none-destroy"]));
        let mut zfs = rec.adapter(&["none-members"]);
        assert_eq!(
            zfs.members(NONE, "t"),
            Err(gone("none-members")),
            "{}",
            rec.image
        );
        assert_eq!(ran(&mut zfs), confirmed(&["none-members"]));

        // `zfs hold`, `holds` and `release` say `dataset does not exist`
        // of a pool that does not: the snapshot is not there.
        let mut zfs = rec.adapter(&["none-hold"]);
        assert_eq!(
            zfs.hold(NONE, "t", TAG),
            Err(ZfsError::NoSnapshot),
            "{}",
            rec.image
        );
        // `held` reads the holds under the snapshot (`holds -r`): none,
        // so no member is read.
        let mut zfs = rec.adapter(&["none-holds-r"]);
        assert_eq!(
            zfs.held(NONE, "t", TAG, None),
            Ok(Held::No),
            "{}",
            rec.image
        );
        assert_eq!(ran(&mut zfs), rec.words(&["none-holds-r"]), "{}", rec.image);
        let mut zfs = rec.adapter(&["none-release"]);
        assert_eq!(zfs.release(NONE, "t", TAG), Ok(()), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["none-release"]), "{}", rec.image);
    }
}

#[test]
fn missing_pool_name_from_a_pool_that_exists_is_an_error() {
    let rec = &recordings()[0];
    let mut zfs = rec.adapter(&["none-get"]);
    zfs.host().ok(
        &format!("zpool status -P {NONE}"),
        &rec.get("rig-status").stdout,
    );
    match zfs.props(NONE) {
        Err(ZfsError::Other(why)) => assert!(why.contains("missing pool name"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn holds_read_as_recorded() {
    for rec in recordings() {
        for pool in [RIG, MIRROR] {
            let name = |what: &str| {
                let prefix = if pool == RIG { "rig" } else { "mirror" };
                format!("{prefix}-{what}")
            };
            let names = [
                name("hold-1"),
                name("hold-2"),
                name("members"),
                name("holds-r"),
            ];
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut zfs = rec.adapter(&names);
            assert_eq!(zfs.hold(pool, "t", TAG), Ok(()), "{} {pool}", rec.image);
            // The tag is there already, and every member has it (the
            // members and their holds as recorded): the hold is done.
            assert_eq!(zfs.hold(pool, "t", TAG), Ok(()), "{} {pool}", rec.image);
            assert_eq!(ran(&mut zfs), rec.words(&names));

            // The members read in the round, then `held` reads the tagged
            // snapshots under the root: every member's `@t` carries the
            // tag. Another tag is on no member: `Held::No`.
            let read = [name("members"), name("holds-r")];
            let read: Vec<&str> = read.iter().map(String::as_str).collect();
            let mut zfs = rec.adapter(&read);
            let members = zfs.members(pool, "t").expect("the members");
            assert!(members.is_some(), "{} {pool}", rec.image);
            assert_eq!(
                zfs.held(pool, "t", TAG, members.as_deref()),
                Ok(Held::All),
                "{} {pool}",
                rec.image
            );
            assert_eq!(
                zfs.held(pool, "t", "another", members.as_deref()),
                Ok(Held::No),
                "{} {pool}",
                rec.image
            );
            let mut both = rec.words(&read);
            both.push(rec.words(&read[1..])[0].clone());
            assert_eq!(ran(&mut zfs), both);

            let release = name("release-1");
            let mut zfs = rec.adapter(&[release.as_str()]);
            assert_eq!(zfs.release(pool, "t", TAG), Ok(()), "{} {pool}", rec.image);
            assert_eq!(ran(&mut zfs), rec.words(&[release.as_str()]));

            // `no such tag`, and then every member still shows the tag (the
            // holds recorded before the release): the release failed.
            let names = [name("release-2"), name("holds-r")];
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let mut zfs = rec.adapter(&names);
            match zfs.release(pool, "t", TAG) {
                Err(ZfsError::Other(why)) => {
                    // By name: `/` sorts before `@`.
                    let kept = format!("{pool}/child@t, {pool}/vol@t, {pool}@t kept the hold");
                    assert!(why.contains(&kept), "{} {pool}: {why}", rec.image);
                }
                other => panic!("{} {pool}: {other:?}", rec.image),
            }
            assert_eq!(ran(&mut zfs), rec.words(&names));
        }
    }
}

#[test]
fn members_read_as_recorded() {
    let expected = [
        // image, pool, the GUIDs of pool@t, child@t, vol@t
        (
            "ubuntu-24.04",
            RIG,
            [
                8905661274482556820,
                8050233510262137147,
                16412086501098145477,
            ],
        ),
        (
            "ubuntu-24.04",
            MIRROR,
            [
                4662300770688396129,
                15588697527660357681,
                12087243843237415635,
            ],
        ),
        (
            "ubuntu-26.04",
            RIG,
            [
                13512717866315419289,
                7783646357815522231,
                9168614937943910373,
            ],
        ),
        (
            "ubuntu-26.04",
            MIRROR,
            [
                11097694641216761050,
                6093465432507679344,
                2749200871953126555,
            ],
        ),
    ];
    let recordings = recordings();
    for (image, pool, snapshot_guids) in expected {
        let rec = recordings
            .iter()
            .find(|r| r.image == image)
            .expect("the image");
        let prefix = if pool == RIG { "rig" } else { "mirror" };
        let name = |what: &str| format!("{prefix}-{what}");

        let members = name("members");
        let mut zfs = rec.adapter(&[members.as_str()]);
        let read = zfs.members(pool, "t");
        let want: Vec<Member> = ["", "/child", "/vol"]
            .iter()
            .zip(snapshot_guids)
            .map(|(suffix, snapshot_guid)| Member {
                name: format!("{pool}{suffix}"),
                snapshot_guid,
            })
            .collect();
        assert_eq!(read, Ok(Some(want)), "{image} {pool}");
        assert_eq!(ran(&mut zfs), rec.words(&[members.as_str()]));
        assert_eq!(zfs.host().ran[0].stdin, MEMBERS_PROGRAM.as_bytes());

        let absent = name("members-absent");
        let mut zfs = rec.adapter(&[absent.as_str()]);
        assert_eq!(zfs.members(pool, "absent"), Ok(None), "{image} {pool}");
        assert_eq!(ran(&mut zfs), rec.words(&[absent.as_str()]));
    }
}

/// The take is `zfs snapshot -r POOL@T` (M2.6c-2b), as recorded on each
/// pool (testing's #152):
/// - taken: exit 0 with nothing on either output, even with
///   `snapshot_limit=0` set on a member (the recorder runs as root);
/// - a member's `@p` taken before: `dataset already exists` for it, then
///   `no snapshots were created` (the list after shows no other `@p`);
/// - every `@t` there: one such line for each member, in an order that
///   varies between runs.
///
/// The last two are [`ZfsError::Exists`]. The adapter's words are the
/// record's. Mutant: `EEXIST` read as another error.
#[test]
fn the_take_as_recorded() {
    for rec in recordings() {
        for pool in [RIG, MIRROR] {
            let prefix = if pool == RIG { "rig" } else { "mirror" };
            for (what, t, want) in [
                ("cli-limit", "q", Ok(())),
                ("cli-exists-child", "p", Err(ZfsError::Exists)),
                ("cli-exists-root", "t", Err(ZfsError::Exists)),
            ] {
                let name = format!("{prefix}-{what}");
                let mut zfs = rec.adapter(&[name.as_str()]);
                assert_eq!(zfs.snapshot(pool, t), want, "{} {name}", rec.image);
                assert_eq!(ran(&mut zfs), rec.words(&[name.as_str()]));
            }
            let after = rec.get(&format!("{prefix}-cli-exists-child-after")).stdout;
            let p: Vec<&str> = after.lines().filter(|l| l.ends_with("@p")).collect();
            assert_eq!(p, [format!("{pool}/child@p")], "{}", rec.image);
        }
    }
}

/// `zfs destroy -r POOL@T` as recorded on the rig's pool: destroyed;
/// no such snapshot (`could not find any snapshots to destroy`), which
/// is destroyed; and a member's `@t` with a hold, which is an error, and
/// destroyed none (the list after). ZFS 2.2's userland (24.04) says
/// `dataset is busy` of the hold, and 2.4.1's (26.04) `it's being held`. Mutants: no
/// such snapshot read as an error, a hold read as destroyed.
#[test]
fn the_destroy_as_recorded() {
    for rec in recordings() {
        let mut zfs = rec.adapter(&["rig-destroy"]);
        assert_eq!(zfs.destroy(RIG, "o0"), Ok(()), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["rig-destroy"]));

        let mut zfs = rec.adapter(&["rig-destroy-absent"]);
        assert_eq!(zfs.destroy(RIG, "absent"), Ok(()), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["rig-destroy-absent"]));

        let mut zfs = rec.adapter(&["rig-destroy-held"]);
        let says = if rec.image == "ubuntu-24.04" {
            "dataset is busy"
        } else {
            "it's being held"
        };
        match zfs.destroy(RIG, "o2") {
            Err(ZfsError::Other(why)) => assert!(
                why.contains(&format!("cannot destroy snapshot skzq/child@o2: {says}")),
                "{}: {why}",
                rec.image
            ),
            other => panic!("{}: {other:?}", rec.image),
        }
        assert_eq!(ran(&mut zfs), rec.words(&["rig-destroy-held"]));
        let after = rec.get("rig-destroy-held-after").stdout;
        for kept in ["skzq@o2", "skzq/child@o2", "skzq/vol@o2"] {
            assert!(after.lines().any(|l| l == kept), "{}: {after}", rec.image);
        }
    }
}

/// A hold on some members' `@t` only (TL-T2 of #147's review), as
/// recorded on the rig's pool. On the root's alone, `zfs hold -r` says
/// `tag already exists` and holds none, so the hold is refused, naming
/// the members that lack it, and `held` reads a part. On the child's
/// alone (the recorder releases the root's first), `zfs release -r` of
/// the pool's `@t` exits 0 and `held` reads none after it. A release of
/// a hold on the root alone is not recorded. Mutants: a part read as
/// held, the refusal not naming the members.
#[test]
fn a_hold_on_a_part_of_the_take_as_recorded() {
    for rec in recordings() {
        let names = ["rig-hold-r-partial", "rig-members", "rig-holds-r-partial"];
        let mut zfs = rec.adapter(&names);
        match zfs.hold(RIG, "t", TAG) {
            Err(ZfsError::Other(why)) => assert!(
                why.ends_with("the tag is on some members and not on skzq/child@t, skzq/vol@t"),
                "{}: {why}",
                rec.image
            ),
            other => panic!("{}: {other:?}", rec.image),
        }
        assert_eq!(ran(&mut zfs), rec.words(&names));

        let read = ["rig-members", "rig-holds-r-partial"];
        let mut zfs = rec.adapter(&read);
        let members = zfs.members(RIG, "t").expect("the members");
        assert_eq!(
            zfs.held(RIG, "t", TAG, members.as_deref()),
            Ok(Held::Part),
            "{}",
            rec.image
        );
        assert_eq!(ran(&mut zfs), rec.words(&read));

        let mut zfs = rec.adapter(&["rig-release-r-partial"]);
        assert_eq!(zfs.release(RIG, "t", TAG), Ok(()), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["rig-release-r-partial"]));
        let mut zfs = rec.adapter(&["rig-holds-r-after-partial"]);
        assert_eq!(
            zfs.held(RIG, "t", TAG, members.as_deref()),
            Ok(Held::No),
            "{}",
            rec.image
        );
    }
}

#[test]
fn the_full_recordings_are_of_the_versions_the_docs_name() {
    for (rec, versions) in full_recordings().iter().zip([
        ["zfs-2.2.2-", "zfs-kmod-2.4.1-"],
        ["zfs-2.4.1-", "zfs-kmod-2.4.1-"],
    ]) {
        let version = rec.get("version").stdout;
        let lines: Vec<&str> = version.lines().collect();
        assert_eq!(lines.len(), 2, "{}: {version}", rec.image);
        for (line, prefix) in lines.iter().zip(versions) {
            assert!(line.starts_with(prefix), "{}: {version}", rec.image);
        }
    }
}

/// The count is `zfs list -r` of the file systems and volumes, as
/// recorded (T3 of #153's review): each pool's root, `child` and `vol`,
/// within the count's own limit. A pool that does not exist says
/// `dataset does not exist`, exit 1: a failed count, which leaves the
/// bases. Mutants: a line not counted, the root not counted, a failed
/// count read as none.
///
/// The snapshots' count that follows a dataset count (R1 of #157's
/// review) was not recorded: here it fails, which the count does not
/// pass on, and the unit tests in `tests.rs` give it its output.
#[test]
fn the_count_as_recorded() {
    let base = Limits::default();
    for rec in full_recordings() {
        for pool in [RIG, MIRROR] {
            let prefix = if pool == RIG { "rig" } else { "mirror" };
            let count = format!("{prefix}-count");
            let mut zfs = rec.adapter(&[count.as_str()]);
            assert_eq!(zfs.count(pool), Ok(3), "{} {pool}", rec.image);
            let mut words = rec.words(&[count.as_str()]);
            words.push(format!("zfs list -r -H -o name -t snapshot {pool}"));
            assert_eq!(ran(&mut zfs), words);
            assert_eq!(zfs.host().ran[0].limit, base.count);
        }
        let mut zfs = rec.adapter(&["none-count"]);
        assert!(zfs.count(NONE).is_err(), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["none-count"]), "no snapshots");
    }
}

/// The pairs pass that the values pass `values` goes with: each line's
/// name and property, the lines that continue a value left out. `zfs
/// get` prints both passes from one walk of the same properties; a value
/// read alone (`alone`, by name and property) shows how many lines it
/// has. A value not read alone is taken to have one line.
fn pairs_of(values: &str, alone: &BTreeMap<(String, String), String>) -> String {
    let mut pairs = String::new();
    let mut lines = values.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let mut columns = line.splitn(3, '\t');
        let (Some(name), Some(property), Some(_)) =
            (columns.next(), columns.next(), columns.next())
        else {
            panic!("not a line of the values pass: {line:?}");
        };
        pairs.push_str(&format!("{name}\t{property}\n"));
        let continued = alone
            .get(&(name.to_string(), property.to_string()))
            .map_or(0, |value| value.matches('\n').count().saturating_sub(1));
        for _ in 0..continued {
            lines.next();
        }
    }
    pairs
}

/// The manifest's read on the recorded pools (T1 and B1 of #157's
/// review): each odd value is what the value read alone gives, a value
/// that holds a newline included (`nl`, `forge`, which forges a line of
/// `child`); the longest value (T4: [`VALUE_MOST`] is 8191, and a longer
/// one cannot be set) is read; the datasets a forged line puts in doubt
/// are read again alone; and `filesystem_count` and
/// `snapshot_count`, which the recording lists as local on every
/// dataset under the limit once set on `child`, are left out, while
/// `snapshot_limit` is kept. The values pass's words are the record's
/// with its options in the adapter's order. The adapter is given the
/// recorded bytes, and the bytes that are not UTF-8 come out exact
/// (N7 of #157's review). Mutants: either counter kept, the limit
/// dropped, the bound one more or one less, the values pass given as
/// text.
#[test]
fn the_manifests_read_as_recorded() {
    assert_eq!(VALUE_MOST, 8191);
    for recording in parse(&READ_IMAGES) {
        for (pool, prefix) in [(RIG, "rig"), (MIRROR, "mirror")] {
            let image = recording.image;
            let get = |what: &str| recording.get(&format!("{prefix}-odd-{what}"));
            let (list, values) = (get("list"), get("get-local"));
            assert!(
                values.stdout.contains("\tsnapshot_count\t"),
                "{image} {pool}"
            );
            assert!(
                values.stdout.contains("\tfilesystem_count\t"),
                "{image} {pool}"
            );
            let mut alone = BTreeMap::new();
            let mut alone_given = BTreeMap::new();
            for odd in ODD {
                let record = get(&format!("value-{odd}"));
                assert_eq!(record.rc, 0, "{image} {pool} {odd}");
                let key = (format!("{pool}/odd-{odd}"), "skyzfs:v".to_string());
                alone_given.insert(key.clone(), given(&record));
                alone.insert(key, record.stdout);
            }
            // The pairs are made from the text, whose names and
            // properties are exact; the values the adapter reads are the
            // recorded bytes (N7 of #157's review).
            let pairs = pairs_of(&values.stdout, &alone);
            let get_words = |columns: &str| {
                format!("zfs get -r -H -p -s local -t filesystem,volume -o {columns} all {pool}")
            };
            let sorted = |words: &str| {
                let mut words: Vec<String> = words.split(' ').map(String::from).collect();
                words.sort();
                words
            };
            assert_eq!(
                sorted(&values.words()),
                sorted(&get_words("name,property,value")),
                "{image} {pool}"
            );
            let mut fake = Fake::default();
            fake.ok(&list.words(), &list.stdout);
            fake.ok(&get_words("name,property"), &pairs);
            fake.give(&get_words("name,property,value"), Ok(given(&values)));
            for ((name, property), output) in &alone_given {
                let words = format!("zfs get -H -p -o value {property} {name}");
                fake.give(&words, Ok(output.clone()));
            }
            // A value not recorded alone is one line of the values pass;
            // read alone, it is that line's value, its bytes exact.
            let line_value = |line: &str| -> Vec<u8> {
                let prefix = format!("{line}\t");
                values
                    .stdout_bytes
                    .split(|&b| b == b'\n')
                    .find_map(|l| l.strip_prefix(prefix.as_bytes()))
                    .unwrap_or_else(|| panic!("{image}: no value of {line}"))
                    .to_vec()
            };
            for line in pairs.lines() {
                let (name, property) = line.split_once('\t').unwrap_or((line, ""));
                if alone.contains_key(&(name.to_string(), property.to_string())) {
                    continue;
                }
                let mut value = line_value(line);
                value.push(b'\n');
                let mut out = output(0, "", "");
                out.stdout = value;
                let words = format!("zfs get -H -p -o value {property} {name}");
                fake.give(&words, Ok(out));
            }
            let mut zfs = ZfsCli::new(fake, Limits::default());
            let datasets = zfs
                .datasets(pool, "g")
                .unwrap_or_else(|e| panic!("{image} {pool}: {e:?}"));
            let alone_read: Vec<String> = ran(&mut zfs).into_iter().skip(3).collect();
            for dataset in &datasets {
                for counter in ["filesystem_count", "snapshot_count"] {
                    assert!(
                        !dataset.local.contains_key(counter),
                        "{image} {}: {counter}",
                        dataset.name
                    );
                }
            }
            let local = |name: &str| {
                &datasets
                    .iter()
                    .find(|d| d.name == name)
                    .unwrap_or_else(|| panic!("{image}: no {name}"))
                    .local
            };
            assert!(
                local(&format!("{pool}/child")).contains_key("snapshot_limit"),
                "{image} {pool}"
            );
            assert!(
                !local(&format!("{pool}/child")).contains_key("skyzfs:forged"),
                "{image} {pool}"
            );
            for odd in ODD {
                let name = format!("{pool}/odd-{odd}");
                let value = local(&name).get("skyzfs:v");
                if odd == "len8192" {
                    assert_eq!(value, None, "{image} {name}");
                    continue;
                }
                let recorded = &alone_given[&(name.clone(), "skyzfs:v".to_string())].stdout;
                let want = recorded.strip_suffix(b"\n").unwrap_or(recorded);
                assert_eq!(value.map(Vec::as_slice), Some(want), "{image} {name}");
            }
            // The bytes that are not UTF-8 are kept exact, in the value
            // read alone (`odd-raw`) and in one taken from the values
            // pass (`child`, read again alone after the forged line).
            let raw = b"a\xff\xfeb\xc3".as_slice();
            assert_eq!(
                local(&format!("{pool}/odd-raw"))["skyzfs:v"],
                raw,
                "{image} {pool}"
            );
            assert_eq!(
                local(&format!("{pool}/child"))["skyzfs:raw"],
                raw,
                "{image} {pool}"
            );
            assert_eq!(
                local(&format!("{pool}/odd-len8191"))["skyzfs:v"].len(),
                VALUE_MOST,
                "{image} {pool}"
            );
            // The forged line names `child`, so `child`, `odd-forge` and
            // `odd-nl` are read again alone, and no counter is.
            for odd in ["forge", "nl"] {
                let words = format!("zfs get -H -p -o value skyzfs:v {pool}/odd-{odd}");
                assert!(alone_read.contains(&words), "{image}: {alone_read:?}");
            }
            let child = format!("zfs get -H -p -o value snapshot_limit {pool}/child");
            assert!(alone_read.contains(&child), "{image}: {alone_read:?}");
            for words in &alone_read {
                assert!(
                    !words.contains("_count "),
                    "{image}: a counter read alone: {words}"
                );
            }
        }
    }
}

/// The output a record gives, its bytes exact.
fn given(record: &Record) -> crate::host::Output {
    let mut given = output(record.rc, "", &record.stderr);
    given.stdout = record.stdout_bytes.clone();
    given
}

/// The manifest's read of [`READ_POOL`] in `state` on a fake that gives
/// the three reads as recorded, a value read alone as `alone` gives it,
/// and any other value read alone as its line of the recorded values
/// pass; with the commands run after the three reads.
fn read_recorded(
    recording: &Recording,
    state: &str,
    alone: &BTreeMap<(String, String), Record>,
) -> (Result<Vec<Dataset>, ZfsError>, Vec<String>) {
    let mut fake = Fake::default();
    let names = recording.get(&format!("read-{state}-get-names"));
    let values = recording.get(&format!("read-{state}-get-values"));
    for read in ["list", "get-names", "get-values"] {
        let record = recording.get(&format!("read-{state}-{read}"));
        fake.give(&record.words(), Ok(given(&record)));
    }
    for line in names.stdout.lines() {
        let (name, property) = line.split_once('\t').unwrap_or((line, ""));
        let words = format!("zfs get -H -p -o value {property} {name}");
        if let Some(record) = alone.get(&(name.to_string(), property.to_string())) {
            fake.give(&words, Ok(given(record)));
            continue;
        }
        let prefix = format!("{line}\t");
        let value = values
            .stdout_bytes
            .split(|b| *b == b'\n')
            .find_map(|l| l.strip_prefix(prefix.as_bytes()))
            .unwrap_or_else(|| panic!("{}: no value of {line}", recording.image));
        let mut out = output(0, "", "");
        out.stdout = [value, b"\n"].concat();
        fake.give(&words, Ok(out));
    }
    let mut zfs = ZfsCli::new(fake, Limits::default());
    let read = zfs.datasets(READ_POOL, "t");
    let after = ran(&mut zfs).into_iter().skip(3).collect();
    (read, after)
}

/// The manifest's read on testing's second recording (#158), whose pairs
/// pass is recorded too: the tree (the root, a child, a volume, all with
/// `@t`, and a file system created after it) is read as one whole take
/// of three members, with the agent's own property left out; the pool
/// with `@t` destroyed has none; each odd value is the value read alone,
/// its bytes exact; the counters are left out where a limit is set, the
/// limits kept; the pairs that [`pairs_of`] makes from the values pass
/// are the recorded pairs pass; and a pool that does not exist is
/// [`ZfsError::NoPool`], where a dataset that goes with it while a value
/// is read alone is an error that names it (N5 of #157's review).
#[test]
fn the_three_reads_as_recorded() {
    for recording in parse(&CLI_READ_IMAGES) {
        let image = recording.image;
        let none = BTreeMap::new();
        let local = |datasets: &[Dataset], name: &str| -> BTreeMap<String, Vec<u8>> {
            datasets
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("{image}: no {name}"))
                .local
                .clone()
        };
        let set = |pairs: &[(&str, &str)]| -> BTreeMap<String, Vec<u8>> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect()
        };

        // The tree.
        let (read, after) = read_recorded(&recording, "tree", &none);
        let tree = read.unwrap_or_else(|e| panic!("{image} tree: {e:?}"));
        assert!(after.is_empty(), "{image}: {after:?}");
        let mut names: Vec<&str> = tree.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["skzrd", "skzrd/child", "skzrd/late", "skzrd/vol"],
            "{image}"
        );
        let root = &tree
            .iter()
            .find(|d| d.name == READ_POOL)
            .expect("root")
            .local;
        assert_eq!(
            root.get("compression").map(Vec::as_slice),
            Some(&b"off"[..])
        );
        assert!(!root.contains_key(ASSIST_PROPERTY), "{image}");
        assert_eq!(
            local(&tree, "skzrd/child"),
            set(&[("skyzfs:note", "local")])
        );
        assert_eq!(
            local(&tree, "skzrd/vol").keys().collect::<Vec<_>>(),
            ["refreservation", "volsize"],
            "{image}"
        );
        match crate::zfs::taken(&tree, READ_POOL) {
            crate::zfs::Taken::Whole(program) => {
                let members: Vec<&str> = program.members.iter().map(|m| m.name.as_str()).collect();
                assert_eq!(members, ["skzrd", "skzrd/child", "skzrd/vol"], "{image}");
            }
            other => panic!("{image} tree: {other:?}"),
        }

        // `@t` destroyed.
        let (read, _) = read_recorded(&recording, "not", &none);
        let not = read.unwrap_or_else(|e| panic!("{image} not: {e:?}"));
        assert!(not.iter().all(|d| d.at.is_none()), "{image}");
        assert_eq!(
            crate::zfs::taken(&not, READ_POOL),
            crate::zfs::Taken::Absent
        );

        // The limits.
        let (read, after) = read_recorded(&recording, "counts", &none);
        let counts = read.unwrap_or_else(|e| panic!("{image} counts: {e:?}"));
        assert!(after.is_empty(), "{image}: {after:?}");
        assert!(recording
            .get("read-counts-get-values")
            .stdout
            .contains("skzrd/child\tsnapshot_count\t"));
        assert_eq!(
            local(&counts, "skzrd/child"),
            set(&[
                ("filesystem_limit", "100"),
                ("skyzfs:note", "local"),
                ("snapshot_limit", "100"),
            ]),
            "{image}"
        );

        // The odd values.
        let mut alone = BTreeMap::new();
        for odd in ODD {
            let Some(record) = recording.records.get(&format!("read-odd-value-{odd}")) else {
                continue;
            };
            assert!(record.is_object());
            let record = recording.get(&format!("read-odd-value-{odd}"));
            assert_eq!(record.rc, 0, "{image} {odd}");
            alone.insert(
                (format!("{READ_POOL}/odd-{odd}"), "skyzfs:v".to_string()),
                record,
            );
        }
        assert_eq!(alone.len(), 11, "{image}");
        let values = recording.get("read-odd-get-values");
        let texts: BTreeMap<(String, String), String> = alone
            .iter()
            .map(|(k, r)| (k.clone(), r.stdout.clone()))
            .collect();
        assert_eq!(
            pairs_of(&values.stdout, &texts),
            recording.get("read-odd-get-names").stdout,
            "{image}"
        );
        let (read, after) = read_recorded(&recording, "odd", &alone);
        let odd = read.unwrap_or_else(|e| panic!("{image} odd: {e:?}"));
        for ((name, _), record) in &alone {
            let want = record
                .stdout_bytes
                .strip_suffix(b"\n")
                .unwrap_or(&record.stdout_bytes);
            assert_eq!(
                local(&odd, name).get("skyzfs:v").map(Vec::as_slice),
                Some(want),
                "{image} {name}"
            );
        }
        assert_eq!(
            local(&odd, "skzrd/odd-len8191")["skyzfs:v"].len(),
            VALUE_MOST,
            "{image}"
        );
        assert!(local(&odd, "skzrd/odd-raw")["skyzfs:v"]
            .iter()
            .any(|b| *b > 0x7f));
        assert_eq!(local(&odd, "skzrd/child"), set(&[("skyzfs:note", "local")]));
        for words in [
            "zfs get -H -p -o value skyzfs:v skzrd/odd-forge",
            "zfs get -H -p -o value skyzfs:v skzrd/odd-nl",
            "zfs get -H -p -o value skyzfs:note skzrd/child",
        ] {
            assert!(after.iter().any(|w| w == words), "{image}: {after:?}");
        }

        // A pool that does not exist, and a dataset gone while a value is
        // read alone.
        let none_list = recording.get("read-none-list");
        let mut fake = Fake::default();
        fake.give(&none_list.words(), Ok(given(&none_list)));
        // Not recorded: no pool, as `zpool list` would list it.
        fake.ok(POOLS, "");
        let mut zfs = ZfsCli::new(fake, Limits::default());
        assert_eq!(
            zfs.datasets(READ_POOL, "t"),
            Err(ZfsError::NoPool(none_list.words())),
            "{image}"
        );
        let gone = recording.get("read-none-value");
        assert_eq!(
            gone.words(),
            "zfs get -H -p -o value skyzfs:note skzrd/child"
        );
        let mut alone_gone = alone.clone();
        alone_gone.insert(("skzrd/child".to_string(), "skyzfs:note".to_string()), gone);
        match read_recorded(&recording, "odd", &alone_gone).0 {
            Err(ZfsError::Other(why)) => assert!(
                why.contains("skzrd/child") && why.contains("dataset does not exist"),
                "{image}: {why}"
            ),
            other => panic!("{image}: {other:?}"),
        }
    }
}
