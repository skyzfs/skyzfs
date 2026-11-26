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
//! pools of the first runs by their GUIDs.
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
use crate::zfs::{premise, taken, Taken, HOLD_TAG};

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

/// The rig's pool and the mirror's.
const RIG: &str = "skzq";
const MIRROR: &str = "skzqm";
/// The pool that does not exist.
const NONE: &str = "skznone";
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
struct Record {
    cmd: String,
    rc: i32,
    stdout: String,
    stderr: String,
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
fn a_hold_on_the_root_alone_as_recorded() {
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
#[test]
fn the_count_as_recorded() {
    let base = Limits::default();
    for rec in full_recordings() {
        for pool in [RIG, MIRROR] {
            let prefix = if pool == RIG { "rig" } else { "mirror" };
            let count = format!("{prefix}-count");
            let mut zfs = rec.adapter(&[count.as_str()]);
            assert_eq!(zfs.count(pool), Ok(3), "{} {pool}", rec.image);
            assert_eq!(ran(&mut zfs), rec.words(&[count.as_str()]));
            assert_eq!(zfs.host().ran[0].limit, base.count);
        }
        let mut zfs = rec.adapter(&["none-count"]);
        assert!(zfs.count(NONE).is_err(), "{}", rec.image);
        assert_eq!(ran(&mut zfs), rec.words(&["none-count"]));
    }
}

/// The members program in full, as recorded on each pool (T3 of #153's
/// review), read through the adapter and judged by [`taken`]:
///
/// - after `zfs snapshot -r POOL@t`: three datasets, each with an `@t` of
///   one group, a whole take whose members are the root, `child` and
///   `vol`; a GUID above 2^63 written negative; each dataset's local
///   properties (the volume's `volsize` and `refreservation`);
/// - of an `@t` that no dataset has: none has one, and the take is
///   [`Taken::Absent`];
/// - of an `@p` that `child` alone has: [`Taken::Absent`] too, since the
///   root has none (`pool@p` does not exist);
/// - of values that are not printable ASCII, in the locale `C` and in
///   `C.UTF-8`: the same bytes, sent in hex;
/// - of a pool that does not exist: [`ZfsError::NoPool`].
///
/// After a count, the program runs within the full read's limit, not a
/// read's. Mutants: a GUID read as signed, an `@t`'s group read from the
/// dataset's, hex values taken as text, the full read given a read's
/// limit.
#[test]
fn the_full_read_as_recorded() {
    let base = Limits::default();
    let raw: &[u8] = &[0x61, 0xff, 0xfe, 0x62, 0xc3];
    let utf8 = "na\u{ef}ve, \u{65e5}\u{672c}, \u{2205}".as_bytes();
    for rec in full_recordings() {
        for pool in [RIG, MIRROR] {
            let prefix = if pool == RIG { "rig" } else { "mirror" };
            let name = |what: &str| format!("{prefix}-{what}");
            let at = |what: &str| format!("{pool}{what}");
            let context = format!("{} {pool}", rec.image);

            let (count, full) = (name("count"), name("members-full"));
            let mut zfs = rec.adapter(&[count.as_str(), full.as_str()]);
            assert_eq!(zfs.count(pool), Ok(3), "{context}");
            let datasets = zfs.datasets(pool, "t").expect("read in full");
            assert_eq!(ran(&mut zfs), rec.words(&[count.as_str(), full.as_str()]));
            assert_eq!(zfs.host().ran[1].stdin, MEMBERS_PROGRAM.as_bytes());
            assert_eq!(
                zfs.host().ran[1].limit,
                base.read + base.full_per_dataset * 3,
                "{context}"
            );
            let mut names: Vec<&str> = datasets.iter().map(|d| d.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, [at(""), at("/child"), at("/vol")], "{context}");
            let get = |name: &str| {
                datasets
                    .iter()
                    .find(|d| d.name == name)
                    .expect("a dataset read")
            };
            let (root, child, vol) = (get(&at("")), get(&at("/child")), get(&at("/vol")));
            assert_eq!(
                (root.createtxg, child.createtxg, vol.createtxg),
                (1, 9, 10),
                "{context}"
            );
            assert_eq!(
                (root.kind, child.kind, vol.kind),
                (Kind::Filesystem, Kind::Filesystem, Kind::Volume)
            );
            let local = |d: &Dataset, key: &str| d.local.get(key).cloned();
            assert_eq!(local(root, "compression"), Some(b"off".to_vec()));
            assert_eq!(local(child, "skyzfs:note"), Some(b"local".to_vec()));
            assert!(local(vol, "volsize").is_some() && local(vol, "refreservation").is_some());
            let Taken::Whole(program) = taken(&datasets, pool) else {
                panic!("{context}: not whole");
            };
            let group = root.at.expect("pool@t").createtxg;
            assert_eq!(program.createtxg, group);
            assert!(datasets
                .iter()
                .all(|d| d.at.map(|a| a.createtxg) == Some(group)));
            let members: Vec<(&str, u64, u64)> = program
                .members
                .iter()
                .map(|m| (m.name.as_str(), m.guid, m.snapshot_guid))
                .collect();
            let want: Vec<(&str, u64, u64)> = [root, child, vol]
                .iter()
                .map(|d| (d.name.as_str(), d.guid, d.at.expect("@t").guid))
                .collect();
            assert_eq!(members, want, "{context}");
            if (rec.image, pool) == ("ubuntu-26.04", RIG) {
                // As the record writes them: -5563034144446978196,
                // 2776128052335566094, -1724167257921105538.
                assert_eq!(
                    program
                        .members
                        .iter()
                        .map(|m| m.snapshot_guid)
                        .collect::<Vec<_>>(),
                    [
                        12_883_709_929_262_573_420,
                        2_776_128_052_335_566_094,
                        16_722_576_815_788_446_078
                    ]
                );
                assert_eq!((group, vol.guid), (34, 15_426_089_004_099_088_699));
            }

            for (what, t) in [
                ("members-full-absent", "absent"),
                ("members-full-partial", "p"),
            ] {
                let record = name(what);
                let mut zfs = rec.adapter(&[record.as_str()]);
                let datasets = zfs.datasets(pool, t).expect("read in full");
                assert_eq!(ran(&mut zfs), rec.words(&[record.as_str()]));
                assert_eq!(datasets.len(), 3, "{context} {what}");
                let with: Vec<&str> = datasets
                    .iter()
                    .filter(|d| d.at.is_some())
                    .map(|d| d.name.as_str())
                    .collect();
                let want: Vec<String> = if t == "p" { vec![at("/child")] } else { vec![] };
                assert_eq!(with, want, "{context} {what}");
                assert_eq!(taken(&datasets, pool), Taken::Absent, "{context} {what}");
            }

            for (what, t) in [("members-full-odd", "o0"), ("members-full-odd-utf8", "o2")] {
                let record = name(what);
                let mut zfs = rec.adapter(&[record.as_str()]);
                let datasets = zfs.datasets(pool, t).expect("read in full");
                assert_eq!(ran(&mut zfs), rec.words(&[record.as_str()]));
                let child = datasets
                    .iter()
                    .find(|d| d.name == at("/child"))
                    .expect("child");
                assert_eq!(
                    local(child, "skyzfs:raw").as_deref(),
                    Some(raw),
                    "{context} {what}"
                );
                assert_eq!(
                    local(child, "skyzfs:utf8").as_deref(),
                    Some(utf8),
                    "{context} {what}"
                );
                assert!(
                    matches!(taken(&datasets, pool), Taken::Whole(_)),
                    "{context} {what}"
                );
            }
        }
        let mut zfs = rec.adapter(&["none-members-full"]);
        assert_eq!(
            zfs.datasets(NONE, "t"),
            Err(ZfsError::NoPool),
            "{}",
            rec.image
        );
        assert_eq!(ran(&mut zfs), rec.words(&["none-members-full"]));
    }
}
