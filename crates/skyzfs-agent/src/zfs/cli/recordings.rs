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
//! `child` and `vol`; the hold's tag is `skyzfs`.
//!
//! A `zpool status -P -g` was recorded beside some of the `-P` outputs
//! only. A `-P` output without its own is read with the `-g` output of
//! the same tree recorded nearest to it (`GUIDED`); only the shape of
//! the tree and its GUIDs are taken from it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::host::fake::{output, Fake};
use crate::zfs::premise;

/// The recordings, by image.
const IMAGES: [(&str, &str); 2] = [
    ("ubuntu-24.04", include_str!("recorded/ubuntu-24.04.json")),
    ("ubuntu-26.04", include_str!("recorded/ubuntu-26.04.json")),
];

/// The rig's pool and the mirror's.
const RIG: &str = "skzq";
const MIRROR: &str = "skzqm";
/// The pool that does not exist.
const NONE: &str = "skznone";
/// The tag the recorder held with.
const TAG: &str = "skyzfs";
/// The device number the fake gives the rig's leaf.
const DEVICE: u64 = 0x0103_0000;

/// One image's records.
struct Recording {
    image: &'static str,
    records: serde_json::Map<String, Value>,
}

/// One command as recorded.
struct Record {
    cmd: String,
    rc: i32,
    stdout: String,
    stderr: String,
}

fn recordings() -> Vec<Recording> {
    IMAGES
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
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{}: `{name}` has no `{key}`", self.image))
                .to_string()
        };
        let rc = record
            .get("rc")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("{}: `{name}` has no `rc`", self.image));
        Record {
            cmd: text("cmd"),
            rc: i32::try_from(rc).expect("an exit code"),
            stdout: text("stdout"),
            stderr: text("stderr"),
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
        ZfsCli::new(fake, Limits::default())
    }

    /// The words of `names`' records.
    fn words(&self, names: &[&str]) -> Vec<String> {
        names.iter().map(|name| self.get(name).words()).collect()
    }
}

impl Record {
    /// The command's words as the adapter runs them.
    fn words(&self) -> String {
        let line = self.cmd.trim_end().replace("\\,", ",");
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
            leaf: 10096076759507422227,
            rig_mirror: 9409777588451531924,
            attached: 3909907667470484589,
            mirror: 18128604407801635679,
            m0: 16373095470363891880,
            m1: 2793949496771227188,
        },
        "ubuntu-26.04" => Guids {
            leaf: 16271519747545802741,
            rig_mirror: 18147170327720314223,
            attached: 13055505963219439480,
            mirror: 9501878435615107014,
            m0: 12234803531026141483,
            m1: 1683628236547958467,
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
            1829423435837219548,
            16886306786165328804,
            false,
        ),
        (
            "ubuntu-24.04",
            "rig-get-checkpoint",
            1829423435837219548,
            16886306786165328804,
            true,
        ),
        (
            "ubuntu-24.04",
            "mirror-get",
            5102303689996654980,
            5329410269614274740,
            false,
        ),
        (
            "ubuntu-24.04",
            "mirror-get-checkpoint",
            5102303689996654980,
            5329410269614274740,
            true,
        ),
        (
            "ubuntu-26.04",
            "rig-get",
            4270366141832664214,
            2506374143759571741,
            false,
        ),
        (
            "ubuntu-26.04",
            "rig-get-checkpoint",
            4270366141832664214,
            2506374143759571741,
            true,
        ),
        (
            "ubuntu-26.04",
            "mirror-get",
            16476376150338987236,
            9930067105697095545,
            false,
        ),
        (
            "ubuntu-26.04",
            "mirror-get-checkpoint",
            16476376150338987236,
            9930067105697095545,
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
    // On 26.04 the resilver of two files had ended when `zpool status`
    // was run: its `scan:` line says `resilvered`.
    (
        "mirror-resilver-status",
        "mirror-resilver-status-g",
        [true, false],
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
            let other = usize::from(named.starts_with("rig"));
            assert_eq!(
                (layout.special, layout.dedup, layout.other),
                (0, 0, other),
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
        let mut zfs = rec.adapter(&["none-status"]);
        assert_eq!(zfs.layout(NONE), Err(ZfsError::NoPool), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["none-status"]));

        // ZFS 2.2's `zpool get` says `missing pool name.`: the adapter
        // asks `zpool status`, as recorded beside it.
        let get = if rec.image == "ubuntu-24.04" {
            vec!["none-get", "none-status"]
        } else {
            vec!["none-get"]
        };
        let mut zfs = rec.adapter(&get);
        assert_eq!(zfs.props(NONE), Err(ZfsError::NoPool), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&get), "{}", rec.image);

        // `zfs snapshot -r` of a pool that does not exist waits for the
        // recorder's next run (testing's #152); the channel program it
        // replaces was recorded.
        let mut zfs = rec.adapter(&["none-members"]);
        assert_eq!(
            zfs.members(NONE, "t"),
            Err(ZfsError::NoPool),
            "{}",
            rec.image
        );
        let mut zfs = rec.adapter(&["none-tagged"]);
        assert_eq!(
            zfs.tagged(NONE, TAG),
            Err(ZfsError::NoPool),
            "{}",
            rec.image
        );

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
fn members_and_tagged_read_as_recorded() {
    let expected = [
        // image, pool, the GUIDs of pool@t, child@t, vol@t
        (
            "ubuntu-24.04",
            RIG,
            [
                17353074155095446544,
                13562140079100978298,
                18141824629642732154,
            ],
        ),
        (
            "ubuntu-24.04",
            MIRROR,
            [
                13892159943213334028,
                5615502437729244678,
                7722993079470159888,
            ],
        ),
        (
            "ubuntu-26.04",
            RIG,
            [
                12891517287467393622,
                11768059368557484299,
                3958339386605656179,
            ],
        ),
        (
            "ubuntu-26.04",
            MIRROR,
            [
                9977119690113618384,
                17106774907999532740,
                12892526617983999289,
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

        let tagged = name("tagged");
        let mut zfs = rec.adapter(&[tagged.as_str()]);
        assert_eq!(zfs.tagged(pool, TAG), Ok(BTreeSet::new()), "{image} {pool}");
        assert_eq!(zfs.host().ran[0].stdin, TAGGED_PROGRAM.as_bytes());
        let held = name("tagged-held");
        let mut zfs = rec.adapter(&[held.as_str()]);
        assert_eq!(
            zfs.tagged(pool, TAG),
            Ok(BTreeSet::from(["t".to_string()])),
            "{image} {pool}"
        );
        assert_eq!(ran(&mut zfs), rec.words(&[held.as_str()]));
    }
}

/// The take is `zfs snapshot -r POOL@T` (M2.6c-2b): as recorded, it
/// exits 0 with nothing on either output, and the adapter's words are
/// the record's. The recording of its failures waits for the recorder's
/// next run (testing's #152).
#[test]
fn the_take_as_recorded() {
    for rec in recordings() {
        for pool in [RIG, MIRROR] {
            let prefix = if pool == RIG { "rig" } else { "mirror" };
            let name = format!("{prefix}-snapshot-cli");
            let mut zfs = rec.adapter(&[name.as_str()]);
            assert_eq!(zfs.snapshot(pool, "t"), Ok(()), "{} {pool}", rec.image);
            assert_eq!(ran(&mut zfs), rec.words(&[name.as_str()]));
        }
    }
}
