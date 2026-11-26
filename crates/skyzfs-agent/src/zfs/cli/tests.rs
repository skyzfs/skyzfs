//! The adapter on a fake host: the commands it runs, and its parsers on
//! output written by hand after OpenZFS's `zpool status`, `zpool get`
//! `zfs program -j`, `zfs list -H` and `zfs get -H`. The parsers on output recorded from ZFS are
//! in `recordings.rs`.

use std::path::PathBuf;

use super::*;
use crate::host::fake::Fake;
use crate::manifest;
use crate::zfs::{premise, Held, HOLD_TAG};

const RIG: &str = "  pool: tank
 state: ONLINE
config:

\tNAME           STATE     READ WRITE CKSUM
\ttank           ONLINE       0     0     0
\t  /dev/ublkb0  ONLINE       0     0     0
\tlogs\t
\t  /dev/ublkb1  ONLINE       0     0     0

errors: No known data errors
";

const RIG_GUIDS: &str = "  pool: tank
 state: ONLINE
config:

\tNAME                    STATE     READ WRITE CKSUM
\ttank                    ONLINE       0     0     0
\t  13451239785512345678  ONLINE       0     0     0
\tlogs\t
\t  775                   ONLINE       0     0     0

errors: No known data errors
";

const BUSY: &str = "  pool: tank
 state: DEGRADED
status: One or more devices is currently being resilvered.  The pool will
\tcontinue to function, possibly in a degraded state.
action: Wait for the resilver to complete.
  scan: resilver in progress since Wed Nov 25 10:00:00 2026
\t1.20G / 4.00G scanned at 300M/s, 600M / 4.00G issued at 150M/s
\t600M resilvered, 15.00% done, 00:00:23 to go
config:

\tNAME                STATE     READ WRITE CKSUM
\ttank                DEGRADED     0     0     0
\t  mirror-0          DEGRADED     0     0     0
\t    /dev/sdb        ONLINE       0     0     0
\t    replacing-1     DEGRADED     0     0     0
\t      /dev/sdc      OFFLINE      0     0     0
\t      /dev/sdd      ONLINE       0     0     0  (resilvering)
\t  raidz2-1          ONLINE       0     0     0
\t    /var/f1         ONLINE       0     0     0
\t    /var/f2         ONLINE       0     0     0
\t    /var/f3         ONLINE       0     0     0
\t  draid1:2d:4c:0s-2 ONLINE       0     0     0
\t    /dev/sdi        ONLINE       0     0     0
\t  spare-4           ONLINE       0     0     0
\t    /dev/sdj        ONLINE       0     0     0
\t    /dev/sdh        ONLINE       0     0     0
\t  4022              UNAVAIL      0     0     0  was /dev/sdk
\tspecial\t
\t  /dev/sde          ONLINE       0     0     0
\tdedup\t
\t  /dev/sdf          ONLINE       0     0     0
\tcache
\t  /dev/sdg          ONLINE       0     0     0
\tspares
\t  /dev/sdh          INUSE     currently in use

errors: No known data errors
";

/// `text` as `zpool status -g` shows it: each vdev's name below the
/// root replaced by a GUID, 1001 and on in the order shown.
fn guided(text: &str) -> String {
    let mut next = 1000;
    let mut in_tree = false;
    let mut out = String::new();
    for line in text.lines() {
        let mut line = line.to_string();
        if line.trim() == "config:" {
            in_tree = true;
        } else if in_tree && line.trim().is_empty() && out.contains("NAME") {
            in_tree = false;
        } else if in_tree {
            if let Some(body) = line.strip_prefix("\t  ") {
                let name = body.split_whitespace().next().unwrap_or_default();
                next += 1;
                line = line.replacen(name, &next.to_string(), 1);
            }
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn cli(fake: Fake) -> ZfsCli<Fake> {
    ZfsCli::new(fake, Limits::default())
}

fn rig() -> Fake {
    let mut fake = Fake::default();
    fake.ok("zpool status -P tank", RIG)
        .ok("zpool status -P -g tank", RIG_GUIDS);
    fake.nodes
        .insert(PathBuf::from("/dev/ublkb0"), Node::Block(0x0103_0000));
    fake.nodes
        .insert(PathBuf::from("/dev/ublkb1"), Node::Block(0x0103_0001));
    fake
}

fn leaf(guid: u64, rdev: Option<u64>) -> Vdev {
    Vdev {
        kind: VdevKind::Disk,
        guid,
        rdev,
        children: Vec::new(),
    }
}

/// The rigs' pool: one leaf on the volume, a log beside it. Its layout
/// reads as one disk of the volume's device, with its GUID from `-g`,
/// and passes the premise. Mutants: the GUID not taken from `-g`, the
/// device not read, a log counted as data.
#[test]
fn the_rigs_pool_reads_as_one_disk_and_passes_the_premise() {
    let mut zfs = cli(rig());
    let layout = zfs.layout("tank").expect("read");
    assert_eq!(
        layout,
        Layout {
            data: vec![leaf(13_451_239_785_512_345_678, Some(0x0103_0000))],
            special: 0,
            dedup: 0,
            other: 1,
            busy: None,
            removed: None,
        }
    );
    let props = Props {
        guid: 1,
        load_guid: 2,
        multihost: false,
        checkpoint: false,
        readonly: false,
    };
    assert_eq!(
        premise(&layout, &props, false, 0x0103_0000, "tank", None),
        Ok(13_451_239_785_512_345_678)
    );
    assert_eq!(
        zfs.host().words(),
        ["zpool status -P tank", "zpool status -P -g tank"]
    );
    assert!(zfs
        .host()
        .ran
        .iter()
        .all(|r| r.limit == Limits::default().read));
}

/// Every kind of vdev and every class reads as it is: interior vdevs by
/// their names, leaves by `stat` (a block device, a file, a path that
/// cannot be read, a leaf named by its GUID), the classes counted, and a
/// resilver under way is busy. Mutants: a kind misread, a class not
/// counted, the scan line not read.
#[test]
fn every_kind_of_vdev_and_class_reads_as_it_is() {
    let mut fake = Fake::default();
    fake.ok("zpool status -P tank", BUSY)
        .ok("zpool status -P -g tank", &guided(BUSY));
    for (path, rdev) in [
        ("/dev/sdb", 0x810),
        ("/dev/sdd", 0x830),
        ("/dev/sdi", 0x880),
    ] {
        fake.nodes.insert(PathBuf::from(path), Node::Block(rdev));
    }
    for path in ["/dev/sdj", "/dev/sdh"] {
        fake.nodes.insert(PathBuf::from(path), Node::Block(0x900));
    }
    for path in ["/var/f1", "/var/f2", "/var/f3"] {
        fake.nodes.insert(PathBuf::from(path), Node::File);
    }
    // /dev/sdc is not there: a disk whose device is not known.
    let layout = cli(fake).layout("tank").expect("read");
    let file = |guid| Vdev {
        kind: VdevKind::File,
        guid,
        rdev: None,
        children: Vec::new(),
    };
    let interior = |kind, guid, children| Vdev {
        kind,
        guid,
        rdev: None,
        children,
    };
    assert_eq!(
        layout.data,
        vec![
            interior(
                VdevKind::Mirror,
                1001,
                vec![
                    leaf(1002, Some(0x810)),
                    interior(
                        VdevKind::Replacing,
                        1003,
                        vec![leaf(1004, None), leaf(1005, Some(0x830))]
                    ),
                ]
            ),
            interior(
                VdevKind::Raidz,
                1006,
                vec![file(1007), file(1008), file(1009)]
            ),
            interior(VdevKind::Draid, 1010, vec![leaf(1011, Some(0x880))]),
            interior(
                VdevKind::Spare,
                1012,
                vec![leaf(1013, Some(0x900)), leaf(1014, Some(0x900))]
            ),
            interior(VdevKind::Other("4022".into()), 1015, vec![]),
        ]
    );
    assert_eq!((layout.special, layout.dedup, layout.other), (1, 1, 2));
    assert_eq!(
        layout.busy.as_deref(),
        Some("scan: resilver in progress since Wed Nov 25 10:00:00 2026")
    );
}

/// A scrub under way is not busy; a removal or a rebuild under way is,
/// and one finished is not; any `remove:` line, under way or done, is a
/// removal (the lead's L4 on #147). Mutants: any scan line taken as busy,
/// the removal line not read, a finished removal not kept.
#[test]
fn only_a_resilver_rebuild_or_removal_under_way_is_busy() {
    let with = |line: &str| RIG.replace(" state: ONLINE\n", &format!(" state: ONLINE\n{line}\n"));
    for (line, busy, removed) in [
        (
            "  scan: scrub in progress since Wed Nov 25 10:00:00 2026",
            false,
            false,
        ),
        (
            "  scan: scrub repaired 0B in 00:00:01 with 0 errors on Wed Nov 25",
            false,
            false,
        ),
        (
            "  scan: resilvered 1.2G in 00:01:00 with 0 errors on Wed Nov 25",
            false,
            false,
        ),
        (
            "  scan: rebuild in progress since Wed Nov 25 10:00:00 2026",
            true,
            false,
        ),
        // A sequential rebuild, as `zpool status` words it.
        (
            "  scan: resilver (draid1:2d:4c:0s-0) in progress since Wed Nov 25",
            true,
            false,
        ),
        (
            "remove: Evacuation of /dev/sdb in progress since Wed Nov 25",
            true,
            true,
        ),
        (
            "remove: Removal of vdev 1 copied 1G in 0h1m, completed on Wed",
            false,
            true,
        ),
    ] {
        let status = parse_status(&with(line), "tank").expect("parsed");
        assert_eq!(status.busy.is_some(), busy, "{line}");
        assert_eq!(
            status.removed.as_deref(),
            removed.then(|| line.trim()),
            "{line}"
        );
    }
}

/// What is not a pool's status is refused: another pool, no config, no
/// heads, an odd indent, an unknown class, a vdev with no parent.
#[test]
fn a_status_that_cannot_be_read_is_refused() {
    let odd = RIG.replace("\t  /dev/ublkb0", "\t   /dev/ublkb0");
    let class = RIG.replace("\tlogs\t", "\tlogz\t");
    let orphan = RIG.replace("\t  /dev/ublkb0", "\t      /dev/ublkb0");
    let untabbed = RIG.replace("\t  /dev/ublkb0", "  /dev/ublkb0");
    for (text, why) in [
        (RIG.replace("pool: tank", "pool: other"), "not `tank`"),
        (RIG.replace("config:", "konfig:"), "no `NAME`"),
        (RIG.replace("\tNAME", "\tNOM"), "no `NAME`"),
        (odd, "odd indent"),
        (class, "unknown class"),
        (orphan, "no parent"),
        (untabbed, "not indented by a tab"),
        ("".into(), "no `pool:`"),
    ] {
        let e = parse_status(&text, "tank").expect_err(why);
        assert!(e.contains(why), "{e}");
    }
}

/// Two reads of the layout that differ in shape are refused, not zipped
/// wrong. Mutant: the shapes not compared.
#[test]
fn a_layout_that_changed_between_its_two_reads_is_refused() {
    let mut fake = rig();
    let grown = RIG_GUIDS.replace("\tlogs\t\n", "\t  776  ONLINE 0 0 0\n\tlogs\t\n");
    fake.commands.remove("zpool status -P -g tank");
    fake.ok("zpool status -P -g tank", &grown);
    let e = cli(fake).layout("tank").expect_err("changed");
    assert!(e.to_string().contains("changed between two reads"), "{e}");
}

/// A pool that is not imported is [`ZfsError::NoPool`], from the
/// message of any command. Mutant: the message not read.
#[test]
fn no_such_pool_is_no_pool() {
    let mut fake = Fake::default();
    fake.fails(
        "zpool status -P tank",
        1,
        "cannot open 'tank': no such pool\n",
    );
    assert_eq!(cli(fake).layout("tank"), Err(ZfsError::NoPool));
}

const PROPS: &str =
    "guid\t17293822569102704640\nload_guid\t42\nmultihost\toff\nreadonly\toff\ncheckpoint\t-\n";

/// The properties read as they are; a checkpoint is `-` or `0` for none
/// and a size for one (this module's reading). Mutants: a property
/// misread, a checkpoint of `-` taken as one.
#[test]
fn the_properties_read_as_they_are() {
    let mut fake = Fake::default();
    fake.ok(
        "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint tank",
        PROPS,
    );
    let props = cli(fake).props("tank").expect("read");
    assert_eq!(
        props,
        Props {
            guid: 17_293_822_569_102_704_640,
            load_guid: 42,
            multihost: false,
            checkpoint: false,
            readonly: false,
        }
    );
    let on = PROPS
        .replace("multihost\toff", "multihost\ton")
        .replace("readonly\toff", "readonly\ton");
    let on = parse_props(&on).expect("read");
    assert!(on.multihost && on.readonly);
    for (value, checkpoint) in [("-", false), ("0", false), ("1048576", true)] {
        let text = PROPS.replace("checkpoint\t-", &format!("checkpoint\t{value}"));
        assert_eq!(parse_props(&text).map(|p| p.checkpoint), Ok(checkpoint));
    }
    let negative = PROPS.replace("load_guid\t42", "load_guid\t-2");
    assert_eq!(
        parse_props(&negative).map(|p| p.load_guid),
        Ok(u64::MAX - 1)
    );
    for bad in [
        PROPS.replace("multihost\toff", "multihost\tmaybe"),
        PROPS.replace("checkpoint\t-", "checkpoint\tsome"),
        PROPS.replace("guid\t17293822569102704640\n", ""),
        PROPS.replace("load_guid\t42", "load_guid 42"),
        format!("{PROPS}readonly\ton\n"),
    ] {
        assert!(parse_props(&bad).is_err(), "{bad}");
    }
}

/// `zfs_nocacheflush` is read from its file. Mutant: its value ignored.
#[test]
fn nocacheflush_is_read_from_the_module() {
    for (text, want) in [("0\n", Ok(false)), ("1\n", Ok(true))] {
        let mut fake = Fake::default();
        fake.files.insert(PathBuf::from(NOCACHEFLUSH), text.into());
        assert_eq!(cli(fake).nocacheflush(), want);
    }
    let mut fake = Fake::default();
    fake.files
        .insert(PathBuf::from(NOCACHEFLUSH), b"x\n".to_vec());
    assert!(cli(fake).nocacheflush().is_err());
    assert!(cli(Fake::default()).nocacheflush().is_err());
}

const TAKE: &str = "zfs snapshot -r tank@skz-1";

/// The take is `zfs snapshot -r POOL@T`, within the limit of a change;
/// exit 0 is taken. `dataset already exists` is [`ZfsError::Exists`];
/// no pool is [`ZfsError::NoPool`]; any other failure is an error that
/// names the command. Mutants: `EEXIST` read as another error, a
/// failure read as taken.
#[test]
fn the_take_is_zfs_snapshot_r() {
    let mut fake = Fake::default();
    fake.ok(TAKE, "");
    let mut zfs = cli(fake);
    assert_eq!(zfs.snapshot("tank", "skz-1"), Ok(()));
    assert_eq!(zfs.host().words(), [TAKE]);
    assert_eq!(zfs.host().ran[0].limit, Limits::default().change);
    assert!(zfs.host().ran[0].stdin.is_empty());
    for (stderr, want) in [
        (
            "cannot create snapshot 'tank/db@skz-1': dataset already exists\nno snapshots were created\n",
            Err(ZfsError::Exists),
        ),
        ("cannot open 'tank': no such pool\n", Err(ZfsError::NoPool)),
    ] {
        let mut fake = Fake::default();
        fake.fails(TAKE, 1, stderr);
        assert_eq!(cli(fake).snapshot("tank", "skz-1"), want, "{stderr}");
    }
    let mut fake = Fake::default();
    fake.fails(
        TAKE,
        1,
        "cannot create snapshot 'tank@skz-1': out of space\nno snapshots were created\n",
    );
    assert!(matches!(
        cli(fake).snapshot("tank", "skz-1"),
        Err(ZfsError::Other(why)) if why.contains(TAKE) && why.contains("out of space")
    ));
}

const LIST_WORDS: &str =
    "zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg tank";
const PAIRS_WORDS: &str =
    "zfs get -r -H -p -s local -t filesystem,volume -o name,property all tank";
const VALUES_WORDS: &str =
    "zfs get -r -H -p -s local -t filesystem,volume -o name,property,value all tank";

/// `zfs list` of the pool: other snapshots, before `@t` and after it,
/// which are no `@t`; a dataset created after the take, with none; a GUID above 2^63.
const LISTED: &str = "tank\t11\tfilesystem\t1
tank@skz-0\t5\tsnapshot\t700
tank@skz-1\t12\tsnapshot\t812
tank/db\t21\tfilesystem\t30
tank/db@skz-1\t22\tsnapshot\t812
tank/db@skz-2\t23\tsnapshot\t820
tank/new\t41\tfilesystem\t813
tank/vol\t18446744073709551614\tvolume\t40
tank/vol@skz-1\t33\tsnapshot\t812
";

const PAIRS: &str = "tank\tcompression\ntank\tskyzfs:assist\ntank/vol\tvolsize\n";

const VALUES: &str =
    "tank\tcompression\tlz4\ntank\tskyzfs:assist\t7\ntank/vol\tvolsize\t1073741824\n";

/// A fake with the manifest's three reads, giving `pairs` and `values`.
fn manifest_reads(pairs: &str, values: &[u8]) -> Fake {
    let mut fake = Fake::default();
    fake.ok(LIST_WORDS, LISTED);
    fake.ok(PAIRS_WORDS, pairs);
    fake.give(
        VALUES_WORDS,
        Ok(Output {
            code: Some(0),
            stdout: values.to_vec(),
            stderr: Vec::new(),
        }),
    );
    fake
}

/// The words of a value read alone.
fn alone(property: &str, name: &str) -> String {
    format!("zfs get -H -p -o value {property} {name}")
}

/// The pool's datasets are read by `zfs list` and twice `zfs get`, in
/// that order, within the full read's limit (the lead's ruling on
/// M2.6c-5); the other snapshots are not `@t`, the agent's own property
/// is left out, and a dataset with no `@t` has none. One whole take of
/// them makes the manifest's members, root first and then by name.
/// Mutants: the property kept, the other snapshot taken for `@t`, the
/// order, a read past the three.
#[test]
fn the_datasets_read_make_the_manifests_members() {
    let mut zfs = cli(manifest_reads(PAIRS, VALUES.as_bytes()));
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(zfs.host().words(), [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS]);
    for ran in &zfs.host().ran {
        assert_eq!(ran.limit, Limits::default().read, "{}", ran.words);
        assert!(ran.stdin.is_empty());
    }
    let new = datasets
        .iter()
        .find(|d| d.name == "tank/new")
        .expect("tank/new");
    assert_eq!((new.createtxg, new.at), (813, None));
    let local = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>()
    };
    let crate::zfs::Taken::Whole(program) = crate::zfs::taken(&datasets, "tank") else {
        panic!("not whole: {datasets:?}");
    };
    assert_eq!(program.createtxg, 812);
    assert_eq!(
        program.members,
        vec![
            manifest::Member {
                name: "tank".into(),
                kind: Kind::Filesystem,
                guid: 11,
                snapshot_guid: 12,
                local: local(&[("compression", "lz4")]),
            },
            manifest::Member {
                name: "tank/db".into(),
                kind: Kind::Filesystem,
                guid: 21,
                snapshot_guid: 22,
                local: local(&[]),
            },
            manifest::Member {
                name: "tank/vol".into(),
                kind: Kind::Volume,
                guid: u64::MAX - 1,
                snapshot_guid: 33,
                local: local(&[("volsize", "1073741824")]),
            },
        ]
    );
    // The members make a manifest that keeps its rules.
    let manifest = manifest::Manifest {
        pool: "tank".into(),
        pool_guid: 1,
        snapshot: "skz-1".into(),
        createtxg: program.createtxg,
        members: program.members,
    };
    assert!(manifest.encode().is_ok());
}

/// `filesystem_count` and `snapshot_count` are counters, not settings,
/// but libzfs gives them the source `local` wherever ZFS keeps them
/// (once a limit is set on the dataset or above it), so `zfs get -s
/// local` lists them; and `snapshot_count` grows with the take itself.
/// They are left out, from the one pass and where the dataset is read
/// again alone, and are not read alone (Codex on #157). Mutants: either
/// counter kept, a counter read alone.
#[test]
fn the_counters_libzfs_calls_local_are_left_out() {
    let pairs = "tank\tcompression\ntank\tsnapshot_count\ntank\tfilesystem_count\n\
                 tank/db\tsnapshot_limit\ntank/db\tsnapshot_count\ntank/db\ta:b\n";
    // In the one pass every value is plain.
    let values = "tank\tcompression\tlz4\ntank\tsnapshot_count\t3\ntank\tfilesystem_count\t2\n\
                  tank/db\tsnapshot_limit\t10\ntank/db\tsnapshot_count\t1\ntank/db\ta:b\tc\n";
    let mut zfs = cli(manifest_reads(pairs, values.as_bytes()));
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(zfs.host().words(), [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS]);
    assert_eq!(
        local_of(&datasets, "tank").keys().collect::<Vec<_>>(),
        ["compression"]
    );
    assert_eq!(
        local_of(&datasets, "tank/db").keys().collect::<Vec<_>>(),
        ["a:b", "snapshot_limit"]
    );

    // tank/db's value of a:b holds a newline: tank/db is read again
    // alone, and its counter is not read.
    let values = "tank\tcompression\tlz4\ntank\tsnapshot_count\t3\ntank\tfilesystem_count\t2\n\
                  tank/db\tsnapshot_limit\t10\ntank/db\tsnapshot_count\t1\ntank/db\ta:b\tc\nd\n";
    let mut fake = manifest_reads(pairs, values.as_bytes());
    fake.ok(&alone("snapshot_limit", "tank/db"), "10\n");
    fake.ok(&alone("a:b", "tank/db"), "c\nd\n");
    let mut zfs = cli(fake);
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(
        zfs.host().words(),
        [
            LIST_WORDS.to_string(),
            PAIRS_WORDS.to_string(),
            VALUES_WORDS.to_string(),
            alone("snapshot_limit", "tank/db"),
            alone("a:b", "tank/db"),
        ]
    );
    let db = local_of(&datasets, "tank/db");
    assert_eq!(db.keys().collect::<Vec<_>>(), ["a:b", "snapshot_limit"]);
    assert_eq!(db["a:b"], b"c\nd");
}

/// A person's properties named like the counters are settings and are
/// kept: only the two counters' exact names are left out (T5 of #157's
/// review). Mutants: a filter widened to names that end `_count`, or
/// that contain a counter's name.
#[test]
fn a_persons_properties_named_like_the_counters_are_kept() {
    let pairs = "tank\tcom.example:snapshot_count\ntank\tx:filesystem_count\n\
                 tank\tsnapshot_count\ntank\tx:y_count\n";
    let values = "tank\tcom.example:snapshot_count\t7\ntank\tx:filesystem_count\t8\n\
                  tank\tsnapshot_count\t3\ntank\tx:y_count\t9\n";
    let mut zfs = cli(manifest_reads(pairs, values.as_bytes()));
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(zfs.host().words(), [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS]);
    let tank = local_of(&datasets, "tank");
    assert_eq!(
        tank.keys().collect::<Vec<_>>(),
        [
            "com.example:snapshot_count",
            "x:filesystem_count",
            "x:y_count"
        ]
    );
    assert_eq!(tank["com.example:snapshot_count"], b"7");
    assert_eq!(tank["x:filesystem_count"], b"8");
}

/// The local properties of `name` in `datasets`.
fn local_of<'a>(datasets: &'a [Dataset], name: &str) -> &'a BTreeMap<String, Vec<u8>> {
    &datasets
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .local
}

/// A value holds any byte but NUL: a tab, bytes that are not UTF-8 or
/// not printable, a value empty, all as `zfs get -H` prints them, read
/// in one pass where no value holds a newline. Mutants: a line split at
/// every tab, the value made UTF-8, a value read again where none needs
/// it.
#[test]
fn values_with_tabs_and_any_bytes_are_read_as_they_are() {
    let pairs = "tank\ta:tab\ntank\ta:bin\ntank\ta:empty\ntank\ta:tabs\n";
    let mut values = Vec::new();
    values.extend_from_slice(b"tank\ta:tab\tx\ty\n");
    values.extend_from_slice(b"tank\ta:bin\t\xff\x01\xe9 \x7f\n");
    values.extend_from_slice(b"tank\ta:empty\t\n");
    values.extend_from_slice(b"tank\ta:tabs\t\t\t\n");
    let mut zfs = cli(manifest_reads(pairs, &values));
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(zfs.host().words(), [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS]);
    let local = local_of(&datasets, "tank");
    assert_eq!(local["a:tab"], b"x\ty");
    assert_eq!(local["a:bin"], b"\xff\x01\xe9 \x7f");
    assert_eq!(local["a:empty"], b"");
    assert_eq!(local["a:tabs"], b"\t\t");
    assert_eq!(local.len(), 4);
}

/// A value that holds a newline cuts its line, and what follows is a
/// line of its own, which may even read as another dataset's property
/// (the lead's ruling on M2.6c-5). The datasets whose lines cannot be
/// trusted are read again one value at a time, with a read's limit;
/// the others are taken from the one pass. Each case: the pairs, the
/// values as printed, the values read alone, the datasets read again.
/// Mutants: no read alone, the line before a suspect one not read
/// again, a pair twice not suspect, a pair missing not read again, the
/// order not compared, or compared by dataset only, the last newline not
/// taken off, all of it taken off.
#[test]
fn a_value_with_a_newline_is_read_again_alone() {
    struct Case {
        what: &'static str,
        pairs: &'static str,
        values: &'static [u8],
        alone: &'static [(&'static str, &'static str, &'static [u8])],
        want: &'static [(&'static str, &'static str, &'static [u8])],
    }
    let cases = [
        Case {
            what: "a newline that forges the next dataset's line",
            pairs: "tank\ta:nl\ntank/db\ta:x\ntank/db\ta:y\ntank/vol\tvolsize\n",
            values: b"tank\ta:nl\tone\ntank/db\ta:y\tforged\ntank/db\ta:x\t1\ntank/db\ta:y\t2\ntank/vol\tvolsize\t8192\n",
            alone: &[
                ("tank", "a:nl", b"one\ntank/db\ta:y\tforged\n"),
                ("tank/db", "a:x", b"1\n"),
                ("tank/db", "a:y", b"2\n"),
            ],
            want: &[
                ("tank", "a:nl", b"one\ntank/db\ta:y\tforged"),
                ("tank/db", "a:x", b"1"),
                ("tank/db", "a:y", b"2"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
        Case {
            what: "a value that ends in a newline, and one of two newlines",
            pairs: "tank\ta:end\ntank\ta:two\ntank/vol\tvolsize\n",
            values: b"tank\ta:end\tv\n\ntank\ta:two\t\n\n\ntank/vol\tvolsize\t8192\n",
            alone: &[
                ("tank", "a:end", b"v\n\n"),
                ("tank", "a:two", b"\n\n\n"),
            ],
            want: &[
                ("tank", "a:end", b"v\n"),
                ("tank", "a:two", b"\n\n"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
        Case {
            what: "a line that names a pair not listed",
            pairs: "tank\ta:nl\ntank/vol\tvolsize\n",
            values: b"tank\ta:nl\tx\ntank/zz\ta:q\ty\ntank/vol\tvolsize\t8192\n",
            alone: &[("tank", "a:nl", b"x\ntank/zz\ta:q\ty\n")],
            want: &[
                ("tank", "a:nl", b"x\ntank/zz\ta:q\ty"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
        Case {
            what: "a pair listed and not read (inherited between the gets)",
            pairs: "tank\ta:gone\ntank/vol\tvolsize\n",
            values: b"tank/vol\tvolsize\t8192\n",
            alone: &[("tank", "a:gone", b"-\n")],
            want: &[("tank", "a:gone", b"-"), ("tank/vol", "volsize", b"8192")],
        },
        Case {
            what: "the pairs read in another order",
            pairs: "tank\ta:1\ntank/db\ta:2\ntank/vol\tvolsize\n",
            values: b"tank/db\ta:2\tq\ntank\ta:1\tp\ntank/vol\tvolsize\t8192\n",
            alone: &[("tank", "a:1", b"p\n"), ("tank/db", "a:2", b"q\n")],
            want: &[
                ("tank", "a:1", b"p"),
                ("tank/db", "a:2", b"q"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
        Case {
            what: "one dataset's pairs read in another order (T2 of #157's review)",
            pairs: "tank\ta:1\ntank\ta:2\ntank/vol\tvolsize\n",
            values: b"tank\ta:2\tq\ntank\ta:1\tp\ntank/vol\tvolsize\t8192\n",
            alone: &[("tank", "a:1", b"p\n"), ("tank", "a:2", b"q\n")],
            want: &[
                ("tank", "a:1", b"p"),
                ("tank", "a:2", b"q"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
        Case {
            what: "a line that is not a name, a property and a value",
            pairs: "tank\ta:1\ntank/vol\tvolsize\n",
            values: b"tank\ta:1\tp\nno tab here\ntank/vol\tvolsize\t8192\n",
            alone: &[("tank", "a:1", b"p\nno tab here\n")],
            want: &[
                ("tank", "a:1", b"p\nno tab here"),
                ("tank/vol", "volsize", b"8192"),
            ],
        },
    ];
    for case in cases {
        let mut fake = manifest_reads(case.pairs, case.values);
        for (name, property, out) in case.alone {
            fake.give(
                &alone(property, name),
                Ok(Output {
                    code: Some(0),
                    stdout: out.to_vec(),
                    stderr: Vec::new(),
                }),
            );
        }
        let mut zfs = cli(fake);
        let datasets = zfs.datasets("tank", "skz-1").expect(case.what);
        let mut words = vec![
            LIST_WORDS.to_string(),
            PAIRS_WORDS.to_string(),
            VALUES_WORDS.to_string(),
        ];
        // In the order the pairs are listed.
        for line in case.pairs.lines() {
            let (name, property) = line.split_once('\t').expect("pair");
            if case
                .alone
                .iter()
                .any(|(n, p, _)| *n == name && *p == property)
            {
                words.push(alone(property, name));
            }
        }
        assert_eq!(zfs.host().words(), words, "{}", case.what);
        for ran in &zfs.host().ran[3..] {
            assert_eq!(ran.limit, Limits::default().read, "{}", case.what);
        }
        let mut got = Vec::new();
        for dataset in &datasets {
            for (property, value) in &dataset.local {
                got.push((dataset.name.as_str(), property.as_str(), value.as_slice()));
            }
        }
        assert_eq!(got, case.want, "{}", case.what);
    }
}

/// A value read alone that is not one value and its newline fails the
/// read, and so does a value read alone that fails, even of a dataset
/// that is gone: neither a pool nor a snapshot missing. A value of
/// 8191 bytes, the most there can be, is read. Mutants: the newline not
/// required, the length not bounded, bounded one short, a failure read
/// as no pool.
#[test]
fn a_value_that_cannot_be_read_alone_fails_the_read() {
    let pairs = "tank\ta:nl\n";
    let values = b"tank\ta:nl\tx\ny\n";
    let most = format!("{}\n", "v".repeat(VALUE_MOST));
    let over = format!("{}\n", "v".repeat(VALUE_MOST + 1));
    let gives: [(Output, bool); 5] = [
        (crate::host::fake::output(0, &most, ""), true),
        (crate::host::fake::output(0, "x\ny", ""), false),
        (crate::host::fake::output(0, &over, ""), false),
        (crate::host::fake::output(0, "", ""), false),
        (
            crate::host::fake::output(1, "", "cannot open 'tank': dataset does not exist\n"),
            false,
        ),
    ];
    for (given, readable) in gives {
        let shown = format!("{given:?}");
        let mut fake = manifest_reads(pairs, values);
        fake.give(&alone("a:nl", "tank"), Ok(given));
        let read = cli(fake).datasets("tank", "skz-1");
        if readable {
            let datasets = read.expect("8191 bytes");
            assert_eq!(local_of(&datasets, "tank")["a:nl"].len(), VALUE_MOST);
        } else {
            assert!(matches!(read, Err(ZfsError::Other(_))), "{shown}: {read:?}");
        }
    }
    // A line longer than a value can be is read again alone.
    let long = format!("tank\ta:nl\t{}\n", "v".repeat(VALUE_MOST + 1));
    let mut fake = manifest_reads(pairs, long.as_bytes());
    fake.ok(&alone("a:nl", "tank"), &most);
    let mut zfs = cli(fake);
    assert!(zfs.datasets("tank", "skz-1").is_ok());
    assert_eq!(zfs.host().words().len(), 4);
}

/// After a count, the manifest's three reads take the full read's limit
/// and a value read alone a read's. Mutants: the value alone given the
/// full read's limit, a get given a read's.
#[test]
fn a_value_read_alone_takes_a_reads_limit() {
    let mut fake = manifest_reads("tank\ta:nl\n", b"tank\ta:nl\tx\ny\n");
    fake.ok(COUNT, &listed(1000));
    fake.ok(SNAPSHOTS, "");
    fake.ok(&alone("a:nl", "tank"), "x\ny\n");
    let mut zfs = cli(fake);
    assert_eq!(zfs.count("tank"), Ok(1000));
    let datasets = zfs.datasets("tank", "skz-1").expect("read");
    assert_eq!(local_of(&datasets, "tank")["a:nl"], b"x\ny");
    let base = Limits::default();
    let limits: Vec<Duration> = zfs.host().ran.iter().map(|r| r.limit).collect();
    let full = base.read + base.full_per_dataset * 1000;
    let read = base.read + base.per_dataset * 1000;
    assert_eq!(limits, [base.count, read, full, full, full, read]);
}

/// A tiny generator of the tests' worlds (xorshift64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The matching against its oracle, on worlds of up to four datasets of
/// up to three properties whose values are made of newlines, tabs,
/// bytes that are not UTF-8, and other lines' names and properties:
/// the values as `zfs get -H` prints them, with nothing changed between
/// the gets. Every value taken from the one pass is the dataset's, every
/// pair of a dataset read again is listed, and where no value holds a
/// newline none is read again. Mutants: any rule of [`match_values`]
/// dropped.
#[test]
fn the_values_matched_are_the_values_set() {
    let names = ["tank", "tank/a", "tank/a b", "tank/c"];
    let properties = ["p:x", "p:y", "volsize"];
    let pieces: [&[u8]; 10] = [
        b"\n",
        b"\t",
        b"v",
        b"\xff",
        b"tank/a",
        b"tank/a b",
        b"p:y",
        b"\tp:x\t",
        b"\n\n",
        b"",
    ];
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut read_again = 0;
    for world in 0..20_000 {
        let mut truth: Vec<(String, String, Vec<u8>)> = Vec::new();
        for name in &names[..1 + rng.below(names.len())] {
            for property in &properties[..rng.below(properties.len() + 1)] {
                let mut value = Vec::new();
                for _ in 0..rng.below(5) {
                    value.extend_from_slice(pieces[rng.below(pieces.len())]);
                }
                truth.push(((*name).into(), (*property).into(), value));
            }
        }
        let pairs: Vec<(String, String)> = truth
            .iter()
            .map(|(n, p, _)| (n.clone(), p.clone()))
            .collect();
        let mut out = Vec::new();
        for (name, property, value) in &truth {
            out.extend_from_slice(format!("{name}\t{property}\t").as_bytes());
            out.extend_from_slice(value);
            out.push(b'\n');
        }
        let (read, again) = match_values(&pairs, &out);
        let newline = truth.iter().any(|(_, _, v)| v.contains(&b'\n'));
        if !newline {
            assert!(again.is_empty(), "world {world}: {truth:?}");
        }
        if !again.is_empty() {
            read_again += 1;
        }
        for ((name, property), value) in &read {
            assert!(!again.contains(name), "world {world}");
            let set = truth
                .iter()
                .find(|(n, p, _)| n == name && p == property)
                .unwrap_or_else(|| panic!("world {world}: {name} {property} not set"));
            assert_eq!(value, &set.2, "world {world}: {name} {property}: {truth:?}");
        }
        // Every pair is read: taken here, or its dataset read again.
        for (name, property, _) in &truth {
            assert!(
                again.contains(name) || read.contains_key(&(name.clone(), property.clone())),
                "world {world}: {name} {property} not read: {truth:?}"
            );
        }
    }
    // The worlds reach the reads alone.
    assert!(read_again > 1000, "{read_again}");
}

/// A dataset as [`crate::zfs::taken`] reads it.
fn dataset(name: &str, createtxg: u64, at: Option<u64>) -> Dataset {
    Dataset {
        name: name.into(),
        kind: Kind::Filesystem,
        guid: createtxg + 100,
        createtxg,
        local: BTreeMap::new(),
        at: at.map(|createtxg| Snap {
            guid: createtxg + 1000,
            createtxg,
        }),
    }
}

/// One whole take of the pool (the lead's rule, M2.6c-2b): every `@t` of
/// `pool@t`'s group, and every dataset created in that group or before
/// with one; a dataset created after it is not a member. No `pool@t` is
/// absent, whatever its members have. Mutants: the group not compared,
/// a dataset older than the take, or of its group, without its `@t`
/// taken as whole, the root not looked for.
#[test]
fn a_take_is_whole_where_every_dataset_older_has_its_t_of_one_group() {
    use crate::zfs::Taken;
    let whole = [
        dataset("tank", 1, Some(50)),
        dataset("tank/db", 10, Some(50)),
        dataset("tank/new", 51, None),
        dataset("tank/newer", 60, None),
    ];
    assert!(
        matches!(crate::zfs::taken(&whole, "tank"), Taken::Whole(p) if p.createtxg == 50 && p.members.len() == 2)
    );
    let rows: [(&str, Vec<Dataset>, &str); 6] = [
        (
            "a member's @t of another group",
            vec![
                dataset("tank", 1, Some(50)),
                dataset("tank/db", 10, Some(49)),
            ],
            "not whole",
        ),
        (
            "a dataset older than the take without its @t",
            vec![dataset("tank", 1, Some(50)), dataset("tank/db", 49, None)],
            "not whole",
        ),
        (
            "a dataset of the take's group without its @t",
            vec![dataset("tank", 1, Some(50)), dataset("tank/db", 50, None)],
            "not whole",
        ),
        (
            "no pool@t, a member's @t there",
            vec![dataset("tank", 1, None), dataset("tank/db", 10, Some(50))],
            "absent",
        ),
        ("no pool@t", vec![dataset("tank", 1, None)], "absent"),
        (
            "the root not read",
            vec![dataset("tank/db", 10, Some(50))],
            "not whole",
        ),
    ];
    for (what, datasets, want) in rows {
        let got = match crate::zfs::taken(&datasets, "tank") {
            Taken::Absent => "absent",
            Taken::NotWhole(_) => "not whole",
            Taken::Whole(_) => "whole",
        };
        assert_eq!(got, want, "{what}");
    }
}

/// Output of `zfs list` or of the pairs' get that cannot be read is an
/// error, and nothing is run after it; a pool that does not exist is no
/// pool, whichever of the reads finds it gone. Mutants: a line of the
/// list with fields short or over taken, a number that is not one, a
/// snapshot of a dataset not listed, the pool's root not required, a
/// name listed twice taken, a pair not split at one tab, a pair twice.
#[test]
fn datasets_that_cannot_be_read_are_an_error() {
    let lists: [&[u8]; 9] = [
        b"tank\t11\tfilesystem\n",
        b"tank\t11\tfilesystem\t1\tx\n",
        b"tank\tx\tfilesystem\t1\n",
        b"tank\t11\tfilesystem\t-1\n",
        b"tank\t11\tbookmark\t1\n",
        b"tank\t11\tfilesystem\t1\ntank/x@skz-1\t3\tsnapshot\t9\n",
        b"tank/db\t21\tfilesystem\t30\n",
        b"tank\t11\tfilesystem\t1\ntank\t11\tfilesystem\t1\n",
        b"tank\t11\tfilesystem\t1\ntank/\xff\t3\tfilesystem\t9\n",
    ];
    for list in lists {
        let shown = String::from_utf8_lossy(list);
        let mut fake = Fake::default();
        fake.give(
            LIST_WORDS,
            Ok(Output {
                code: Some(0),
                stdout: list.to_vec(),
                stderr: Vec::new(),
            }),
        );
        let mut zfs = cli(fake);
        assert!(
            matches!(zfs.datasets("tank", "skz-1"), Err(ZfsError::Other(_))),
            "{shown}"
        );
        assert_eq!(zfs.host().words(), [LIST_WORDS], "{shown}");
    }
    for pairs in [
        "tank\n",
        "tank\ta\tb\n",
        "\ta\n",
        "tank\t\n",
        "tank\ta\ntank\ta\n",
    ] {
        let mut zfs = cli(manifest_reads(pairs, b""));
        assert!(
            matches!(zfs.datasets("tank", "skz-1"), Err(ZfsError::Other(_))),
            "{pairs:?}"
        );
        assert_eq!(zfs.host().words(), [LIST_WORDS, PAIRS_WORDS], "{pairs:?}");
    }
    let gone = "cannot open 'tank': dataset does not exist\n";
    for at in 0..3 {
        let mut fake = manifest_reads(PAIRS, VALUES.as_bytes());
        let words = [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS][at];
        fake.commands.remove(words);
        fake.fails(words, 1, gone);
        assert_eq!(
            cli(fake).datasets("tank", "skz-1"),
            Err(ZfsError::NoPool),
            "{at}"
        );
    }
    // A child that does not exist, named by the same message, is no
    // pool that does not exist (N5 of #157's review): an error that
    // names the command and the child.
    for gone in [
        "cannot open 'tank/db': dataset does not exist\n",
        "cannot open 'tank@skz-1': dataset does not exist\n",
        "cannot open 'tank ': dataset does not exist\n",
    ] {
        for at in 0..3 {
            let mut fake = manifest_reads(PAIRS, VALUES.as_bytes());
            let words = [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS][at];
            fake.commands.remove(words);
            fake.fails(words, 1, gone);
            assert!(
                matches!(
                    cli(fake).datasets("tank", "skz-1"),
                    Err(ZfsError::Other(why)) if why.contains(words) && why.contains(gone.trim_end())
                ),
                "{gone} {at}"
            );
        }
    }
    let mut fake = Fake::default();
    fake.fails(LIST_WORDS, 1, "cannot open 'tank': no such pool\n");
    assert_eq!(cli(fake).datasets("tank", "skz-1"), Err(ZfsError::NoPool));
    let mut fake = Fake::default();
    fake.fails(LIST_WORDS, 1, "cannot iterate filesystems: I/O error\n");
    assert!(matches!(
        cli(fake).datasets("tank", "skz-1"),
        Err(ZfsError::Other(why)) if why.contains(LIST_WORDS) && why.contains("I/O error")
    ));
}

/// A dataset that the gets name and the list did not, created between
/// them, is no member of the take and is left out; `@t` of a dataset is
/// found wherever the list prints it. Mutant: a dataset made from the
/// gets alone.
#[test]
fn a_dataset_created_after_the_list_is_left_out() {
    let mut fake = Fake::default();
    fake.ok(
        LIST_WORDS,
        "tank/db@skz-1\t22\tsnapshot\t812\ntank\t11\tfilesystem\t1\ntank/db\t21\tfilesystem\t30\n",
    );
    fake.ok(PAIRS_WORDS, "tank\ta\ntank/later\tb\n");
    fake.ok(VALUES_WORDS, "tank\ta\t1\ntank/later\tb\t2\n");
    let datasets = cli(fake).datasets("tank", "skz-1").expect("read");
    let names: Vec<_> = datasets.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["tank", "tank/db"]);
    assert_eq!(
        datasets[1].at,
        Some(Snap {
            guid: 22,
            createtxg: 812
        })
    );
    assert_eq!(datasets[0].at, None);
    assert_eq!(local_of(&datasets, "tank")["a"], b"1");
}

const DESTROY: &str = "zfs destroy -r tank@skz-1";

/// A take that is not whole, or not as read before it, is destroyed by
/// `zfs destroy -r`; one already gone is destroyed. Any other failure is
/// an error that names the command. Mutants: a take already gone read
/// as an error, a failure read as destroyed.
#[test]
fn the_destroy_is_zfs_destroy_r() {
    for (code, stderr) in [
        (0, ""),
        (
            1,
            "could not find any snapshots to destroy; check snapshot names.\n",
        ),
        (1, "cannot open 'tank': dataset does not exist\n"),
    ] {
        let mut fake = Fake::default();
        if code == 0 {
            fake.ok(DESTROY, "");
        } else {
            fake.fails(DESTROY, code, stderr);
        }
        let mut zfs = cli(fake);
        assert_eq!(zfs.destroy("tank", "skz-1"), Ok(()), "{stderr}");
        assert_eq!(zfs.host().words(), [DESTROY]);
        assert_eq!(zfs.host().ran[0].limit, Limits::default().change);
    }
    let mut fake = Fake::default();
    fake.fails(
        DESTROY,
        1,
        "cannot destroy snapshot tank@skz-1: pool I/O is currently suspended\n",
    );
    assert!(matches!(
        cli(fake).destroy("tank", "skz-1"),
        Err(ZfsError::Other(why)) if why.contains(DESTROY) && why.contains("suspended")
    ));
    // Killed at its limit, the destroy may still finish in the kernel
    // (TL-F3 of #147's review).
    let mut fake = Fake::default();
    fake.give(DESTROY, Err(std::io::ErrorKind::TimedOut));
    assert!(matches!(
        cli(fake).destroy("tank", "skz-1"),
        Err(ZfsError::Other(why))
            if why.starts_with("tank@skz-1 may not have been destroyed: ") && why.contains(DESTROY)
    ));
}

/// `zfs destroy` reads `%` ranges and `,` lists after the `@`: a name
/// that is not a snapshot's name is not destroyed (NL-S1). Mutant: the
/// guard left out.
#[test]
fn what_is_not_a_snapshots_name_is_not_destroyed() {
    for t in ["a%z", "a,b", ""] {
        let mut zfs = cli(Fake::default());
        let Err(ZfsError::Other(why)) = zfs.destroy("tank", t) else {
            panic!("{t}");
        };
        assert!(why.contains("was not destroyed"), "{t}: {why}");
        assert!(zfs.host().words().is_empty(), "{t}");
    }
}

/// A message is read after the line's last `": "`, so that a name
/// holding `": "` and the words looked for cannot say them (NL-T3).
/// Mutant: the first `": "` taken.
#[test]
fn a_name_that_holds_the_words_does_not_say_them() {
    let line = b"cannot hold 'tank/x: no such pool@skz-1': tag already exists\n";
    assert!(says(line, "tag already exists"));
    assert!(!says(line, "no such pool"));
    let line = b"cannot open 'tank/a: b': dataset does not exist\n";
    assert!(says(line, "dataset does not exist"));
    assert!(!says(
        b"cannot open 'tank/dataset does not exist: x': permission denied\n",
        "dataset does not exist"
    ));
}

const MEMBERS_WORDS: &str = "zfs program -n -j -t 100000000 -m 104857600 tank - tank skz-1";

/// Each member's `@t` is read by a read-only program, root first; none
/// where `pool@t` is absent. Mutants: absent read as no members, the
/// program run to change the pool.
#[test]
fn the_members_are_read_by_a_read_only_program() {
    let mut fake = Fake::default();
    fake.ok(
        MEMBERS_WORDS,
        r#"{"return":{"members":{"tank/db":"22","tank":"-12"}}}"#,
    );
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.members("tank", "skz-1"),
        Ok(Some(vec![
            Member {
                name: "tank".into(),
                snapshot_guid: u64::MAX - 11,
            },
            Member {
                name: "tank/db".into(),
                snapshot_guid: 22,
            },
        ]))
    );
    assert_eq!(zfs.host().ran[0].stdin, MEMBERS_PROGRAM.as_bytes());
    assert_eq!(zfs.host().ran[0].limit, Limits::default().read);
    let mut fake = Fake::default();
    fake.ok(MEMBERS_WORDS, r#"{"return":{"absent":"1"}}"#);
    assert_eq!(cli(fake).members("tank", "skz-1"), Ok(None));
    let mut fake = Fake::default();
    fake.ok(MEMBERS_WORDS, r#"{"return":{"members":{"tank/db":"22"}}}"#);
    assert!(cli(fake).members("tank", "skz-1").is_err());
}

const HOLD: &str = "zfs hold -r skyzfs-publish tank@skz-1";
const RELEASE: &str = "zfs release -r skyzfs-publish tank@skz-1";
const HOLDS_R: &str = "zfs holds -H -r tank@skz-1";

/// A hold already there is held only where every member carries the
/// tag: `zfs hold -r` holds nothing when one member has it already, so
/// the members are read and their holds checked. A snapshot that does
/// not exist is [`ZfsError::NoSnapshot`]. Mutants: "tag already exists"
/// an error, or taken as held with no look; a member without the tag
/// missed.
#[test]
fn a_hold_already_there_is_held_where_every_member_has_it() {
    const EXISTS: &str =
        "cannot hold snapshot 'tank/db@skz-1': tag already exists on this dataset\n";
    const BOTH: &str = r#"{"return":{"members":{"tank":"11","tank/db":"22"}}}"#;
    let hold = |given: Result<&str, &str>| {
        let mut fake = Fake::default();
        match given {
            Ok(out) => fake.ok(HOLD, out),
            Err(e) => fake.fails(HOLD, 1, e),
        };
        let mut zfs = cli(fake);
        let done = zfs.hold("tank", "skz-1", HOLD_TAG);
        assert_eq!(zfs.host().ran[0].limit, Limits::default().change);
        done
    };
    assert_eq!(hold(Ok("")), Ok(()));
    let tagged = "tank@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n\
                  tank/db@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n";
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, EXISTS)
        .ok(MEMBERS_WORDS, BOTH)
        .ok(HOLDS_R, tagged);
    let mut zfs = cli(fake);
    assert_eq!(zfs.hold("tank", "skz-1", HOLD_TAG), Ok(()));
    assert_eq!(zfs.host().words(), [HOLD, MEMBERS_WORDS, HOLDS_R]);
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, EXISTS).ok(MEMBERS_WORDS, BOTH).ok(
        HOLDS_R,
        "tank/db@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n\
         tank@skz-1\tother\tWed Nov 25 10:00 2026\n",
    );
    let e = cli(fake)
        .hold("tank", "skz-1", HOLD_TAG)
        .expect_err("one member without the tag");
    assert!(e.to_string().contains("tank@skz-1"), "{e}");
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, EXISTS)
        .ok(MEMBERS_WORDS, r#"{"return":{"absent":"1"}}"#);
    assert_eq!(
        cli(fake).hold("tank", "skz-1", HOLD_TAG),
        Err(ZfsError::NoSnapshot)
    );
    assert_eq!(
        hold(Err("cannot open 'tank@skz-1': dataset does not exist\n")),
        Err(ZfsError::NoSnapshot)
    );
    assert!(matches!(
        hold(Err(
            "cannot hold snapshot 'tank@skz-1': pool I/O is currently suspended\n"
        )),
        Err(ZfsError::Other(_))
    ));
}

/// A message is read after the last `": "` of its line, so the name of
/// a dataset (which may hold spaces) is not taken for it. Mutant: the
/// whole line matched (S1).
#[test]
fn the_words_of_a_datasets_name_are_not_its_message() {
    const SUSPENDED: &str = "pool I/O is currently suspended";
    let mut fake = Fake::default();
    fake.fails(
        HOLD,
        1,
        &format!("cannot hold snapshot 'tank/no such pool@skz-1': {SUSPENDED}\n"),
    );
    let e = cli(fake)
        .hold("tank", "skz-1", HOLD_TAG)
        .expect_err("suspended");
    assert!(matches!(e, ZfsError::Other(_)), "{e:?}");
    assert!(e.to_string().contains(SUSPENDED), "{e}");
    let mut fake = Fake::default();
    fake.fails(
        RELEASE,
        1,
        &format!(
            "cannot release hold from snapshot 'tank/dataset does not exist@skz-1': {SUSPENDED}\n"
        ),
    );
    let e = cli(fake)
        .release("tank", "skz-1", HOLD_TAG)
        .expect_err("suspended");
    assert!(matches!(e, ZfsError::Other(_)), "{e:?}");
}

/// No such tag, or no such snapshot, is released, but only where no
/// member keeps the tag. Mutants: "no such tag" taken as released with
/// no look, a member that keeps it missed.
#[test]
fn a_release_without_the_tag_is_released_where_no_member_keeps_it() {
    const NO_TAG: &str =
        "cannot release hold from snapshot 'tank/db@skz-1': no such tag on this dataset\n";
    let mut fake = Fake::default();
    fake.ok(RELEASE, "");
    assert_eq!(cli(fake).release("tank", "skz-1", HOLD_TAG), Ok(()));
    let mut fake = Fake::default();
    fake.fails(
        RELEASE,
        1,
        "cannot open 'tank@skz-1': dataset does not exist\n",
    );
    assert_eq!(cli(fake).release("tank", "skz-1", HOLD_TAG), Ok(()));
    let mut fake = Fake::default();
    fake.fails(RELEASE, 1, NO_TAG)
        .ok(HOLDS_R, "tank@skz-1\tother\tWed Nov 25 10:00 2026\n");
    let mut zfs = cli(fake);
    assert_eq!(zfs.release("tank", "skz-1", HOLD_TAG), Ok(()));
    assert_eq!(zfs.host().words(), [RELEASE, HOLDS_R]);
    let mut fake = Fake::default();
    fake.fails(RELEASE, 1, NO_TAG).ok(
        HOLDS_R,
        "tank@skz-1\tother\tWed Nov 25 10:00 2026\ntank/vol@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n",
    );
    let e = cli(fake)
        .release("tank", "skz-1", HOLD_TAG)
        .expect_err("kept");
    assert!(
        e.to_string().contains("tank/vol@skz-1 kept the hold"),
        "{e}"
    );
}

/// `held` is the tag on every member's `@t`: the root's alone is not
/// enough (NL-F1 of #147's review), and a snapshot that does not exist
/// is not held. Where some members have it and others not, it is a part
/// (TL-F1), which the planner releases. Mutants: any hold taken as the
/// tag, the root's tag taken for every member's, a missing snapshot an
/// error, a part read as none.
#[test]
fn held_is_the_tag_on_every_members_snapshot() {
    const BOTH: &str = r#"{"return":{"members":{"tank":"11","tank/db":"22"}}}"#;
    const ROOT: &str = "tank@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n";
    const DB: &str = "tank/db@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n";
    let held = |holds: Result<&str, &str>, members: Option<&str>| {
        let mut fake = Fake::default();
        match holds {
            Ok(out) => fake.ok(HOLDS_R, out),
            Err(e) => fake.fails(HOLDS_R, 1, e),
        };
        if let Some(members) = members {
            fake.ok(MEMBERS_WORDS, members);
        }
        let mut zfs = cli(fake);
        // The members as the engine reads them in the same round, which
        // `held` does not read again (TL-S2 of #147's review).
        let read = match members {
            Some(_) => zfs.members("tank", "skz-1").expect("the members"),
            None => None,
        };
        let before = zfs.host().words().len();
        let held = zfs.held("tank", "skz-1", HOLD_TAG, read.as_deref());
        let words: Vec<String> = zfs.host().words()[before..]
            .iter()
            .map(|w| w.to_string())
            .collect();
        (held, words)
    };
    let every = format!("{ROOT}{DB}");
    assert_eq!(
        held(Ok(&every), Some(BOTH)),
        (Ok(Held::All), vec![HOLDS_R.to_string()]),
        "the members given are not read again"
    );
    assert_eq!(
        held(Ok(ROOT), Some(BOTH)).0,
        Ok(Held::Part),
        "the root's only: tank/db@skz-1 lacks it"
    );
    assert_eq!(
        held(Ok(DB), None),
        (Ok(Held::Part), vec![HOLDS_R.to_string()]),
        "the root lacks it: a part"
    );
    assert_eq!(
        held(Ok("tank@skz-1\tother\tWed Nov 25 10:00 2026\n"), None).0,
        Ok(Held::No)
    );
    assert_eq!(held(Ok(""), None).0, Ok(Held::No));
    assert_eq!(
        held(
            Err("cannot open 'tank@skz-1': dataset does not exist\n"),
            None
        )
        .0,
        Ok(Held::No)
    );
    assert_eq!(
        held(Ok(&every), Some(r#"{"return":{"absent":"1"}}"#)).0,
        Ok(Held::Part),
        "`pool@t` gone when the members were read: the tags read are a part"
    );
}

const TAGGED_WORDS: &str = "zfs program -n -j -t 100000000 -m 104857600 tank - tank skyzfs-publish";

/// The snapshots with the tag are read by a read-only program, by name;
/// an empty table may come as an empty list. Mutant: the names not read.
#[test]
fn the_tagged_snapshots_are_read_by_name() {
    let mut fake = Fake::default();
    fake.ok(
        TAGGED_WORDS,
        r#"{"return":{"tagged":{"skz-2":"1","skz-1":"1"}}}"#,
    );
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string(), "skz-2".to_string()].into())
    );
    assert_eq!(zfs.host().ran[0].stdin, TAGGED_PROGRAM.as_bytes());
    let mut fake = Fake::default();
    fake.ok(TAGGED_WORDS, r#"{"return":{"tagged":[]}}"#);
    assert_eq!(cli(fake).tagged("tank", HOLD_TAG), Ok(BTreeSet::new()));
    let mut fake = Fake::default();
    fake.ok(TAGGED_WORDS, r#"{"return":{}}"#);
    assert!(cli(fake).tagged("tank", HOLD_TAG).is_err());
}

/// The assist sets the property, then syncs, each within the limit of a
/// change; a set that fails syncs nothing. Mutants: the sync alone, the
/// order reversed.
#[test]
fn the_assist_sets_the_property_then_syncs() {
    let mut fake = Fake::default();
    fake.ok("zfs set skyzfs:assist=7 tank", "")
        .ok("zpool sync tank", "");
    let mut zfs = cli(fake);
    assert_eq!(zfs.assist("tank", 7), Ok(()));
    assert_eq!(
        zfs.host().words(),
        ["zfs set skyzfs:assist=7 tank", "zpool sync tank"]
    );
    assert!(zfs
        .host()
        .ran
        .iter()
        .all(|r| r.limit == Limits::default().change));
    let mut fake = Fake::default();
    fake.fails("zfs set skyzfs:assist=8 tank", 1, "cannot set property\n");
    let mut zfs = cli(fake);
    assert!(zfs.assist("tank", 8).is_err());
    assert_eq!(zfs.host().words(), ["zfs set skyzfs:assist=8 tank"]);
}

/// A command past its limit is an error that names it. Mutant: a
/// timeout read as success.
#[test]
fn a_command_past_its_limit_is_an_error() {
    let mut fake = Fake::default();
    fake.give("zpool sync tank", Err(std::io::ErrorKind::TimedOut))
        .ok("zfs set skyzfs:assist=1 tank", "");
    let e = cli(fake).assist("tank", 1).expect_err("timed out");
    assert!(e.to_string().contains("zpool sync tank"), "{e}");
}

/// The agent's programs only read: the take is `zfs snapshot -r`
/// (M2.6c-2b). And they call nothing the channel programs' sandbox
/// leaves out. Read in their text: the programs themselves run on the
/// rigs (Q10).
#[test]
fn the_programs_only_read_and_call_what_the_sandbox_has() {
    assert!(!MEMBERS_PROGRAM.contains("zfs.sync."));
    assert!(!TAGGED_PROGRAM.contains("zfs.sync."));
    // Nothing the channel programs' sandbox leaves out: on ZFS 2.2 and
    // 2.4 a call of `pcall` failed the program (the recordings of Q10),
    // and the sandbox has no file, operating system or loading functions
    // either (zfs-program(8)).
    for program in [MEMBERS_PROGRAM, TAGGED_PROGRAM] {
        for absent in [
            "pcall",
            "xpcall",
            "print(",
            "require",
            "dofile",
            "loadfile",
            "load(",
            "io.",
            "os.",
            "debug.",
            "coroutine.",
        ] {
            let code: String = program
                .lines()
                .map(|line| line.split("--").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(!code.contains(absent), "a program calls `{absent}`");
        }
    }
}

const COUNT: &str = "zfs list -r -H -o name -t filesystem,volume tank";
const SNAPSHOTS: &str = "zfs list -r -H -o name -t snapshot tank";

/// `n` datasets as `zfs list -r -H -o name` lists them.
fn listed(n: usize) -> String {
    (0..n).map(|i| format!("tank/fs{i}\n")).collect()
}

/// The lead's rules on M2.6c-2b and on N2 of #153's review: the count
/// is `zfs list -r` of the file systems and volumes, within its own
/// fixed limit; a read or a change then takes 100 ms more for each
/// dataset counted, at most 20 minutes, and each of the manifest's three
/// reads 200 ms more on the read's base, at most 35 minutes; a count
/// that fails sets the bases, as before any count. The limit of one of
/// the manifest's reads at 10 000 datasets is more than the whole read
/// took there on Ubuntu 26.04 under a write load (two gets of 468.2 s
/// and a list of 33.8 s, run 45733395008; the lead's ruling on M2.6c-5),
/// and at 10 001 at least twice what the members program in full took
/// (911.8 s, run 45720225770), which it replaced. Mutants: the limits
/// not scaled, scaled but not capped, the manifest's reads scaled or
/// capped as a read, a read scaled as the manifest's, a failed count
/// keeping the last or setting the longest, the count's own limit
/// scaled.
#[test]
fn the_limits_grow_with_the_datasets_counted() {
    let base = Limits::default();
    let s = Duration::from_secs;
    let ms = Duration::from_millis;
    assert_eq!(
        (base.read, base.change, base.count, base.most),
        (s(60), s(120), s(60), s(20 * 60))
    );
    assert_eq!(
        (base.per_dataset, base.full_per_dataset, base.full_most),
        (ms(100), ms(200), s(35 * 60))
    );
    let at = |n: u64, base: Duration, per_ms: u64| base + Duration::from_millis(per_ms * n);
    let mut fake = Fake::default();
    fake.ok(TAKE, "");
    fake.ok(HOLDS_R, "");
    fake.ok(LIST_WORDS, LISTED);
    fake.ok(PAIRS_WORDS, PAIRS);
    fake.ok(VALUES_WORDS, VALUES);
    fake.ok(SNAPSHOTS, "");
    let mut zfs = cli(fake);
    let limits = |zfs: &mut ZfsCli<Fake>| {
        zfs.host().ran.clear();
        let _ = zfs.snapshot("tank", "skz-1");
        let _ = zfs.held("tank", "skz-1", "skyzfs-publish", Some(&[]));
        let _ = zfs.datasets("tank", "skz-1");
        // The other reads take a read's limit, not the full read's (T10
        // of #153's next look).
        let _ = zfs.members("tank", "skz-1");
        let _ = zfs.tagged("tank", "skyzfs-publish");
        let _ = zfs.status("tank", false);
        let ran = &zfs.host().ran;
        assert_eq!(ran[0].words, TAKE);
        assert_eq!(ran[1].words, HOLDS_R);
        assert_eq!(
            ran[2..5]
                .iter()
                .map(|r| r.words.as_str())
                .collect::<Vec<_>>(),
            [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS]
        );
        for manifests in &ran[3..5] {
            assert_eq!(manifests.limit, ran[2].limit, "{}", manifests.words);
        }
        assert_eq!(
            ran[5..]
                .iter()
                .map(|r| r.words.as_str())
                .collect::<Vec<_>>(),
            [MEMBERS_WORDS, TAGGED_WORDS, "zpool status -P tank"]
        );
        for other in &ran[5..] {
            assert_eq!(other.limit, ran[1].limit, "{}", other.words);
        }
        (ran[0].limit, ran[1].limit, ran[2].limit)
    };
    assert_eq!(
        limits(&mut zfs),
        (base.change, base.read, base.read),
        "no count yet"
    );
    let full_read_10001 = Duration::from_millis(911_828);
    assert!(at(10_001, base.read, 200) >= full_read_10001 * 2);
    let manifest_read_10000 = ms(468_190) * 2 + ms(33_797);
    assert!(at(10_000, base.read, 200) > manifest_read_10000);
    assert!(at(10_000, base.read, 200) < base.full_most);
    for (n, change, read, full) in [
        (
            3,
            at(3, base.change, 100),
            at(3, base.read, 100),
            at(3, base.read, 200),
        ),
        (
            5000,
            at(5000, base.change, 100),
            at(5000, base.read, 100),
            at(5000, base.read, 200),
        ),
        (
            10_001,
            at(10_001, base.change, 100),
            at(10_001, base.read, 100),
            at(10_001, base.read, 200),
        ),
        // 60 s + 2060 s is past 35 minutes: the full read reaches its
        // own cap first.
        (
            10_300,
            at(10_300, base.change, 100),
            at(10_300, base.read, 100),
            base.full_most,
        ),
        // 120 s + 1080 s is 20 minutes: the change reaches the cap.
        (
            10_800,
            base.most,
            at(10_800, base.read, 100),
            base.full_most,
        ),
        // 60 s + 1140 s is 20 minutes: the read reaches it.
        (11_400, base.most, base.most, base.full_most),
        (20_000, base.most, base.most, base.full_most),
        (0, base.change, base.read, base.read),
    ] {
        zfs.host().commands.remove(COUNT);
        zfs.host().ok(COUNT, &listed(n as usize));
        zfs.host().ran.clear();
        assert_eq!(zfs.count("tank"), Ok(n));
        assert_eq!(zfs.host().words(), [COUNT, SNAPSHOTS]);
        assert_eq!(zfs.host().ran[0].limit, base.count, "{n}");
        assert_eq!(zfs.host().ran[1].limit, read, "{n}");
        assert_eq!(limits(&mut zfs), (change, read, full), "{n}");
    }
    zfs.host().commands.remove(COUNT);
    zfs.host().ok(COUNT, &listed(7));
    assert_eq!(zfs.count("tank"), Ok(7));
    zfs.host().commands.remove(COUNT);
    zfs.host().fails(
        COUNT,
        1,
        "cannot open 'tank': pool I/O is currently suspended\n",
    );
    zfs.host().ran.clear();
    assert!(zfs.count("tank").is_err());
    assert_eq!(zfs.host().words(), [COUNT], "no snapshots counted");
    assert_eq!(
        limits(&mut zfs),
        (base.change, base.read, base.read),
        "a count failed"
    );
    zfs.host().commands.remove(COUNT);
    zfs.host().ok(COUNT, &listed(2));
    assert_eq!(zfs.count("tank"), Ok(2));
    assert_eq!(
        limits(&mut zfs),
        (
            at(2, base.change, 100),
            at(2, base.read, 100),
            at(2, base.read, 200)
        ),
        "counted again"
    );
}

/// R1 of #157's review, the lead's ruling (a): `zfs list` in the
/// manifest's read lists every snapshot in the pool, so its limit grows
/// with the snapshots counted as well as the datasets: the count also
/// counts the snapshots (`zfs list -r -H -o name -t snapshot`, within a
/// read's limit), and the list takes [`Limits::full_per_dataset`] for
/// each dataset and each snapshot, at most [`Limits::full_most`]. The
/// two gets, which read only file systems and volumes, and every other
/// command keep the datasets' scale. A snapshot count that fails leaves
/// the list on the datasets' scale (the conservative reading: no longer
/// limit than before on a pool whose commands fail), and the count of
/// the datasets stands. Mutants: the snapshots not counted, counted but
/// not added, added to the gets or to the other commands, the list not
/// capped, a failed snapshot count failing the count or setting the
/// bases, the snapshot count within the count's own limit.
#[test]
fn the_lists_limit_grows_with_the_snapshots_counted() {
    let base = Limits::default();
    let at = |n: u64, per_ms: u64| base.read + Duration::from_millis(per_ms * n);
    let snapshots = |n: usize| -> String { (0..n).map(|i| format!("tank@s{i}\n")).collect() };
    let mut fake = Fake::default();
    fake.ok(COUNT, &listed(10));
    fake.ok(LIST_WORDS, LISTED);
    fake.ok(PAIRS_WORDS, PAIRS);
    fake.ok(VALUES_WORDS, VALUES);
    fake.ok(HOLDS_R, "");
    let mut zfs = cli(fake);
    // (snapshots listed, the list's limit)
    for (m, list) in [
        (0, at(10, 200)),
        (1, at(11, 200)),
        (990, at(1000, 200)),
        (20_000, at(20_010, 200)),
        // 60 s + 200 ms × 10 500 is past 35 minutes.
        (10_490, base.full_most),
        (1_000_000, base.full_most),
    ] {
        zfs.host().commands.remove(SNAPSHOTS);
        zfs.host().ok(SNAPSHOTS, &snapshots(m));
        zfs.host().ran.clear();
        assert_eq!(zfs.count("tank"), Ok(10), "{m}");
        assert_eq!(zfs.host().words(), [COUNT, SNAPSHOTS], "{m}");
        assert_eq!(zfs.host().ran[1].limit, at(10, 100), "{m}");
        zfs.host().ran.clear();
        let _ = zfs.datasets("tank", "skz-1");
        let _ = zfs.held("tank", "skz-1", "skyzfs-publish", Some(&[]));
        let ran = &zfs.host().ran;
        let words: Vec<&str> = ran.iter().map(|r| r.words.as_str()).collect();
        assert_eq!(
            words,
            [LIST_WORDS, PAIRS_WORDS, VALUES_WORDS, HOLDS_R],
            "{m}"
        );
        assert_eq!(ran[0].limit, list.min(base.full_most), "{m}");
        assert_eq!(ran[1].limit, at(10, 200), "{m}");
        assert_eq!(ran[2].limit, at(10, 200), "{m}");
        assert_eq!(ran[3].limit, at(10, 100), "{m}");
    }
    zfs.host().commands.remove(SNAPSHOTS);
    zfs.host()
        .fails(SNAPSHOTS, 1, "cannot iterate filesystems: I/O error\n");
    assert_eq!(zfs.count("tank"), Ok(10), "a failed snapshot count");
    zfs.host().ran.clear();
    let _ = zfs.datasets("tank", "skz-1");
    let ran = &zfs.host().ran;
    assert_eq!(ran[0].words, LIST_WORDS);
    assert_eq!(ran[0].limit, at(10, 200), "the datasets' scale");
    assert_eq!(ran[1].limit, at(10, 200));
}
