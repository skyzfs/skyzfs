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

/// What the adapter asks where a command says its pool is not there.
const POOLS: &str = "zpool list -H -o name";

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
    fake.ok(POOLS, "");
    let mut zfs = cli(fake);
    let gone = Err(ZfsError::NoPool("zpool status -P tank".into()));
    assert_eq!(zfs.layout("tank"), gone);
    assert_eq!(zfs.host().words(), ["zpool status -P tank", POOLS]);
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
        (
            "cannot open 'tank': no such pool\n",
            Err(ZfsError::NoPool(TAKE.into())),
        ),
    ] {
        let mut fake = Fake::default();
        fake.fails(TAKE, 1, stderr);
        fake.ok(POOLS, "other\n");
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
    assert_eq!(limits, [base.count, base.full_most, full, full, full, read]);
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
        fake.ok(POOLS, "");
        assert_eq!(
            cli(fake).datasets("tank", "skz-1"),
            Err(ZfsError::NoPool(words.into())),
            "{at}"
        );
        // The same message while `zpool list` lists the pool (a read
        // racing an import or an export) is no missing pool: the pool
        // is there, so the read failed.
        let mut fake = manifest_reads(PAIRS, VALUES.as_bytes());
        fake.commands.remove(words);
        fake.fails(words, 1, gone);
        fake.ok(POOLS, "tank\n");
        assert!(
            matches!(
                cli(fake).datasets("tank", "skz-1"),
                Err(ZfsError::Vanished(why)) if why.contains(words) && why.contains(gone.trim_end())
            ),
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
    fake.ok(POOLS, "");
    assert_eq!(
        cli(fake).datasets("tank", "skz-1"),
        Err(ZfsError::NoPool(LIST_WORDS.into()))
    );
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

/// The holds' check's count of holds after `zfs holds -H -r`, given to
/// `fake` as `holds` implies it, one hold a line: `zfs list -H -p -o
/// name,userrefs -t snapshot` of the snapshots named, by name. Its
/// words, where `holds` names a snapshot.
fn counted(fake: &mut Fake, holds: &str) -> Option<String> {
    let mut refs: BTreeMap<&str, u64> = BTreeMap::new();
    for line in holds.lines() {
        let (name, _) = line.split_once('\t').expect("a hold's line");
        *refs.entry(name).or_default() += 1;
    }
    if refs.is_empty() {
        return None;
    }
    let names: Vec<&str> = refs.keys().copied().collect();
    let words = refs_words(&names);
    let listed: String = refs
        .iter()
        .map(|(name, n)| format!("{name}\t{n}\n"))
        .collect();
    fake.ok(&words, &listed);
    Some(words)
}

/// The words of the holds' check's count of `names`.
fn refs_words(names: &[&str]) -> String {
    format!(
        "zfs list -H -p -o name,userrefs -t snapshot {}",
        names.join(" ")
    )
}

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
    let count = counted(&mut fake, tagged).expect("two held");
    let mut zfs = cli(fake);
    assert_eq!(zfs.hold("tank", "skz-1", HOLD_TAG), Ok(()));
    assert_eq!(
        zfs.host().words(),
        [HOLD, MEMBERS_WORDS, HOLDS_R, count.as_str()]
    );
    let mut fake = Fake::default();
    let one = "tank/db@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n\
               tank@skz-1\tother\tWed Nov 25 10:00 2026\n";
    fake.fails(HOLD, 1, EXISTS)
        .ok(MEMBERS_WORDS, BOTH)
        .ok(HOLDS_R, one);
    counted(&mut fake, one);
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
    let other = "tank@skz-1\tother\tWed Nov 25 10:00 2026\n";
    fake.fails(RELEASE, 1, NO_TAG).ok(HOLDS_R, other);
    let count = counted(&mut fake, other).expect("one held");
    let mut zfs = cli(fake);
    assert_eq!(zfs.release("tank", "skz-1", HOLD_TAG), Ok(()));
    assert_eq!(zfs.host().words(), [RELEASE, HOLDS_R, count.as_str()]);
    let mut fake = Fake::default();
    let kept = "tank@skz-1\tother\tWed Nov 25 10:00 2026\ntank/vol@skz-1\tskyzfs-publish\tWed Nov 25 10:00 2026\n";
    fake.fails(RELEASE, 1, NO_TAG).ok(HOLDS_R, kept);
    counted(&mut fake, kept);
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
            Ok(out) => {
                fake.ok(HOLDS_R, out);
                counted(&mut fake, out);
            }
            Err(e) => {
                fake.fails(HOLDS_R, 1, e);
            }
        }
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
        (
            Ok(Held::All),
            vec![
                HOLDS_R.to_string(),
                refs_words(&["tank/db@skz-1", "tank@skz-1"])
            ]
        ),
        "the members given are not read again"
    );
    assert_eq!(
        held(Ok(ROOT), Some(BOTH)).0,
        Ok(Held::Part),
        "the root's only: tank/db@skz-1 lacks it"
    );
    assert_eq!(
        held(Ok(DB), None),
        (
            Ok(Held::Part),
            vec![HOLDS_R.to_string(), refs_words(&["tank/db@skz-1"])]
        ),
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

const TAGGED_LIST: &str = "zfs list -H -p -r -t snapshot -o name,userrefs tank";

/// The sweep's `zfs holds -H` of `snapshots`.
fn holds_of(snapshots: &[&str]) -> String {
    format!("zfs holds -H {}", snapshots.join(" "))
}

/// A line of `zfs holds -H`: the snapshot, the tag and when it was held.
fn hold_line(snapshot: &str, tag: &str) -> String {
    format!("{snapshot}\t{tag}\tWed Nov 25 21:24 2026\n")
}

/// The sweep lists the pool's snapshots with their count of holds, reads
/// with `zfs holds -H` the holds of those that have one, and keeps the
/// names `t` of those that have the tag, of any dataset: no channel
/// program (S1 of #161's review, the lead's ruling). The list takes the
/// list's limit on the snapshots counted, each `zfs holds` a read's.
/// Mutants: a snapshot with no hold read, a hold of another tag kept
/// (one the agent's begins with, or one that begins with the agent's),
/// the dataset's part kept in the name, the list on a read's limit, the
/// holds on the list's.
#[test]
fn the_sweep_reads_the_holds_of_the_snapshots_that_have_one() {
    let snapshots = "tank@skz-1\t1\ntank/db@skz-1\t1\ntank@none\t0\n\
                  tank/db@mine\t3\ntank/vol@skz-2\t1\n";
    let held = [
        "tank@skz-1",
        "tank/db@skz-1",
        "tank/db@mine",
        "tank/vol@skz-2",
    ];
    let holds = [
        hold_line("tank@skz-1", HOLD_TAG),
        hold_line("tank/db@skz-1", HOLD_TAG),
        hold_line("tank/db@mine", "keep"),
        hold_line("tank/db@mine", "skyzfs-publish-not"),
        hold_line("tank/db@mine", "skyzfs"),
        hold_line("tank/vol@skz-2", HOLD_TAG),
    ]
    .concat();
    let mut fake = Fake::default();
    fake.ok(COUNT, &listed(10))
        .ok(SNAPSHOTS, &"tank@s\n".repeat(1000))
        .ok(TAGGED_LIST, snapshots)
        .ok(&holds_of(&held), &holds);
    let mut zfs = cli(fake);
    assert_eq!(zfs.count("tank"), Ok(10));
    zfs.host().ran.clear();
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string(), "skz-2".to_string()].into())
    );
    let base = Limits::default();
    let read = base.read + base.per_dataset * 10;
    let ran = &zfs.host().ran;
    let words: Vec<&str> = ran.iter().map(|r| r.words.as_str()).collect();
    assert_eq!(words, [TAGGED_LIST, holds_of(&held).as_str()]);
    assert_eq!(
        ran[0].limit,
        read + base.list_per_snapshot * 1000,
        "the list"
    );
    assert_eq!(ran[1].limit, read, "the holds");

    // No snapshot with a hold: the list alone.
    for listed in ["", "tank@a\t0\ntank/db@b\t0\n"] {
        let mut fake = Fake::default();
        fake.ok(TAGGED_LIST, listed);
        let mut zfs = cli(fake);
        assert_eq!(
            zfs.tagged("tank", HOLD_TAG),
            Ok(BTreeSet::new()),
            "{listed:?}"
        );
        assert_eq!(zfs.host().words(), [TAGGED_LIST], "{listed:?}");
    }
    // A line that is not a snapshot and its count cannot be read.
    for listed in ["tank@a\tx\n", "tank\t1\n", "tank@a\n", "tank@a\t-1\n"] {
        let mut fake = Fake::default();
        fake.ok(TAGGED_LIST, listed);
        let read = cli(fake).tagged("tank", HOLD_TAG);
        assert!(
            matches!(&read, Err(ZfsError::Other(why)) if why.contains(TAGGED_LIST)),
            "{listed:?}: {read:?}"
        );
    }
}

/// The sweep names [`HOLDS_AT_ONCE`] snapshots at most to each `zfs
/// holds -H`, in the list's order, where the names are short enough that
/// [`HOLDS_ARG_BYTES`] does not bound them first. Mutants: another bound,
/// all in one command, a chunk left out.
#[test]
fn the_sweep_reads_the_holds_a_thousand_snapshots_at_a_time() {
    assert_eq!(HOLDS_AT_ONCE, 1000);
    let names: Vec<String> = (0..2500).map(|i| format!("tank/d{i}@skz-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let listed: String = names.iter().map(|n| format!("{n}\t1\n")).collect();
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, &listed);
    for chunk in refs.chunks(1000) {
        let holds: String = chunk.iter().map(|n| hold_line(n, HOLD_TAG)).collect();
        fake.ok(&holds_of(chunk), &holds);
    }
    let mut zfs = cli(fake);
    let tagged = zfs.tagged("tank", HOLD_TAG).expect("tagged");
    assert_eq!(tagged.len(), 2500);
    let words = zfs.host().words();
    let chunks: Vec<String> = refs.chunks(1000).map(holds_of).collect();
    assert_eq!(words.len(), 4);
    assert_eq!(words[1..], chunks[..]);
    assert_eq!(
        chunks
            .iter()
            .map(|c| c.split(' ').count() - 3)
            .collect::<Vec<_>>(),
        [1000, 1000, 500]
    );
}

/// 1000 snapshots with names of 255 bytes, the longest ZFS allows, are
/// read 248 a command: each costs 264 bytes against the kernel's limit
/// (its bytes, its NUL, its pointer), and 248 of them fit in
/// [`HOLDS_ARG_BYTES`], 249 do not. 1000 in one command (264 000 bytes)
/// fail with `E2BIG` at `LimitSTACK=1M`, whose limit is 256 KiB
/// (Codex's P2 on #162). Each command, with its own words, stays within
/// half of the kernel's floor of 128 KiB. Mutants: no bound on the
/// bytes, the pointer or the NUL not counted, a bound of 128 KiB.
#[test]
fn the_sweep_bounds_a_holds_command_by_its_bytes() {
    assert_eq!(HOLDS_ARG_BYTES, 64 << 10);
    assert_eq!(arg_cost("tank@t"), 6 + 1 + std::mem::size_of::<usize>());
    let names: Vec<String> = (0..1000)
        .map(|i| {
            let name = format!("tank/{}@skz-{i:04}", "d".repeat(241));
            assert_eq!(name.len(), 255);
            name
        })
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let listed: String = names.iter().map(|n| format!("{n}\t1\n")).collect();
    let per = HOLDS_ARG_BYTES / (255 + 1 + std::mem::size_of::<usize>());
    if std::mem::size_of::<usize>() == 8 {
        assert_eq!(per, 248);
    }
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, &listed);
    for chunk in refs.chunks(per) {
        let holds: String = chunk.iter().map(|n| hold_line(n, HOLD_TAG)).collect();
        fake.ok(&holds_of(chunk), &holds);
    }
    let mut zfs = cli(fake);
    let tagged = zfs.tagged("tank", HOLD_TAG).expect("tagged");
    assert_eq!(tagged.len(), 1000);
    let words = zfs.host().words();
    let chunks: Vec<String> = refs.chunks(per).map(holds_of).collect();
    assert_eq!(words[1..], chunks[..]);
    for command in &words[1..] {
        let cost: usize = command.split(' ').map(arg_cost).sum();
        assert!(cost <= (128 << 10) / 2, "{cost} bytes: {}", &command[..40]);
    }
}

/// [`holds_batches`] fills a command up to [`HOLDS_ARG_BYTES`]
/// exactly, keeps the order, never makes an empty command, and puts a
/// name longer than the bound in a command alone. Mutants: a command
/// cut one name early at the bound, the bytes not begun again with each
/// command, a name too long left out.
#[test]
fn the_holds_batches_fill_up_to_the_bound() {
    assert!(holds_batches(&[] as &[&str]).is_empty());
    // Names that cost 256 bytes each: 256 fill the bound exactly.
    let fits = 256 - 1 - std::mem::size_of::<usize>();
    let names: Vec<String> = (0..600).map(|i| format!("{i:0fits$}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let sizes: Vec<usize> = holds_batches(&refs).iter().map(|b| b.len()).collect();
    assert_eq!(sizes, [256, 256, 88]);
    let again: Vec<&str> = holds_batches(&refs).concat();
    assert_eq!(again, refs);
    let long = "x".repeat(HOLDS_ARG_BYTES);
    let odd = ["a@1", long.as_str(), "b@2"];
    let sizes: Vec<usize> = holds_batches(&odd).iter().map(|b| b.len()).collect();
    assert_eq!(sizes, [1, 1, 1]);
}

/// A snapshot listed with a hold and gone before its holds are read, or
/// a dataset gone during the list, makes the whole read again, at most
/// [`VANISHED_READS`] times in all; a pool that is not there is a
/// missing pool where `zpool list` agrees. Mutants: the holds' `dataset
/// does not exist` taken as no hold, or as an error at once; the list's
/// message of a missing pool not confirmed.
#[test]
fn the_sweep_reads_again_what_vanished_under_it() {
    let both = ["tank@skz-1", "tank@gone"];
    // What `zfs holds -H` gives where one of its snapshots is gone, as
    // read in OpenZFS 2.2's `zfs_do_holds` and not recorded: the holds
    // of the others, the error of the one gone, exit 1 (`no datasets
    // available` only where none had a hold).
    let gone = || {
        Ok(crate::host::fake::output(
            1,
            &hold_line("tank@skz-1", HOLD_TAG),
            "cannot open 'tank@gone': dataset does not exist\n",
        ))
    };
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\ntank@gone\t1\n")
        .ok(TAGGED_LIST, "tank@skz-1\t1\n")
        .give(&holds_of(&both), gone())
        .ok(
            &holds_of(&["tank@skz-1"]),
            &hold_line("tank@skz-1", HOLD_TAG),
        );
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string()].into())
    );
    assert_eq!(
        zfs.host().words(),
        [
            TAGGED_LIST.to_string(),
            holds_of(&both),
            TAGGED_LIST.to_string(),
            holds_of(&["tank@skz-1"])
        ]
    );
    // Gone at every read: the error, after three.
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\ntank@gone\t1\n")
        .give(&holds_of(&both), gone());
    let mut zfs = cli(fake);
    let read = zfs.tagged("tank", HOLD_TAG);
    assert!(
        matches!(&read, Err(ZfsError::Vanished(why)) if why.contains(&holds_of(&both))),
        "{read:?}"
    );
    assert_eq!(zfs.host().words().len(), 6);
    // libzfs's other message of a snapshot gone during the read, where
    // `zpool list` lists the pool: read again (TG-A of #162's review).
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\ntank@gone\t1\n")
        .ok(TAGGED_LIST, "tank@skz-1\t1\n")
        .fails(
            &holds_of(&both),
            1,
            "cannot get holds for 'tank@gone': no such pool or dataset\n",
        )
        .ok(
            &holds_of(&["tank@skz-1"]),
            &hold_line("tank@skz-1", HOLD_TAG),
        )
        .ok(POOLS, "tank\n");
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string()].into())
    );
    assert_eq!(
        zfs.host().words(),
        [
            TAGGED_LIST.to_string(),
            holds_of(&both),
            POOLS.to_string(),
            TAGGED_LIST.to_string(),
            holds_of(&["tank@skz-1"])
        ]
    );
    // A dataset gone during the list, then not.
    let mut fake = Fake::default();
    fake.fails(
        TAGGED_LIST,
        1,
        "cannot iterate filesystems: no such pool or dataset\n",
    )
    .ok(TAGGED_LIST, "")
    .ok(POOLS, "tank\n");
    let mut zfs = cli(fake);
    assert_eq!(zfs.tagged("tank", HOLD_TAG), Ok(BTreeSet::new()));
    assert_eq!(zfs.host().words(), [TAGGED_LIST, POOLS, TAGGED_LIST]);
    // The pool not there: `zfs list` names it.
    for (pools, missing) in [("other\n", true), ("tank\n", false)] {
        let mut fake = Fake::default();
        fake.fails(
            TAGGED_LIST,
            1,
            "cannot open 'tank': dataset does not exist\n",
        )
        .ok(POOLS, pools);
        let read = cli(fake).tagged("tank", HOLD_TAG);
        if missing {
            assert_eq!(read, Err(ZfsError::NoPool(TAGGED_LIST.into())), "{pools:?}");
        } else {
            assert!(
                matches!(read, Err(ZfsError::Vanished(_))),
                "{pools:?}: {read:?}"
            );
        }
    }
}

/// Another holder's tag is read as bytes: one that is not UTF-8 does not
/// fail the sweep (D1 of #162's review). Mutant: the holds read as
/// UTF-8.
#[test]
fn another_holders_tag_is_read_as_bytes() {
    let held = ["tank@skz-1", "tank/db@backup"];
    let mut holds = hold_line("tank@skz-1", HOLD_TAG).into_bytes();
    holds.extend_from_slice(b"tank/db@backup\tcaf\xe9\tWed Nov 25 21:24 2026\n");
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\ntank/db@backup\t1\n")
        .give(&holds_of(&held), Ok(bytes_out(holds)));
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string()].into())
    );
}

/// What a command printed on its standard output, exit 0.
fn bytes_out(stdout: Vec<u8>) -> crate::host::Output {
    crate::host::Output {
        code: Some(0),
        stdout,
        stderr: Vec::new(),
    }
}

/// A tag of another holder's with a tab or a newline, on a snapshot whose
/// holds the sweep reads, makes lines that are not a hold each: one that
/// is not three fields, one of a snapshot not asked, or one forged whole
/// that names a snapshot asked, which the count of holds (`userrefs`)
/// finds. Each is read again, and then is an error that names the
/// snapshot, and not skipped (F1-R of #162's review; the lead's ruling on
/// M2.6c-5e). Mutants: a line skipped, a forged line kept (no count, or
/// the count not compared), no reading again, an output cut short read.
#[test]
fn holds_that_do_not_add_up_are_an_error_that_names_the_snapshot() {
    const WHEN: &[u8] = b"\tWed Nov 25 21:24 2026\n";
    let held = ["tank@skz-1", "tank/db@backup"];
    let agent = hold_line("tank@skz-1", HOLD_TAG).into_bytes();
    // Another holder's hold on tank/db@backup, its tag `tag`.
    let with = |tag: &[u8]| {
        let mut holds = agent.clone();
        holds.extend_from_slice(b"tank/db@backup\t");
        holds.extend_from_slice(tag);
        holds.extend_from_slice(WHEN);
        holds
    };
    let cases: [(&str, Vec<u8>, &str); 6] = [
        (
            "a newline: a line of two fields",
            with(b"x\ntank/db@forged\tskyzfs-publish"),
            "\"tank/db@backup\\tx\"",
        ),
        (
            "two newlines: a line of one field",
            with(b"x\tY\nlonely\nz"),
            "after a hold of `tank/db@backup`: \"lonely\"",
        ),
        (
            "a tab: a line of four fields, the agent's tag second",
            with(b"skyzfs-publish\tz"),
            "\"tank/db@backup\\tskyzfs-publish\\tz\\tWed Nov 25 21:24 2026\"",
        ),
        (
            "a line forged whole, of a snapshot not asked",
            with(b"x\tY\ntank/other@skz-9\tskyzfs-publish"),
            "after a hold of `tank/db@backup`: \"tank/other@skz-9\\tskyzfs-publish",
        ),
        (
            "a line forged whole, of the snapshot itself",
            with(b"x\tY\ntank/db@backup\tskyzfs-publish"),
            "`tank/db@backup` has 2 lines of holds and 1 holds (`userrefs`): a line is \
             forged by a tag with a newline, on the hold just before one of them \
             (of tank/db@backup, tank@skz-1)",
        ),
        (
            "a line forged whole, of another snapshot asked",
            with(b"x\tY\ntank@skz-1\tskyzfs-publish"),
            "`tank@skz-1` has 2 lines of holds and 1 holds (`userrefs`): a line is \
             forged by a tag with a newline, on the hold just before one of them \
             (of tank/db@backup)",
        ),
    ];
    for (case, holds, names) in cases {
        let mut fake = Fake::default();
        fake.ok(TAGGED_LIST, "tank@skz-1\t1\ntank/db@backup\t1\n")
            .give(&holds_of(&held), Ok(bytes_out(holds)));
        let mut zfs = cli(fake);
        match zfs.tagged("tank", HOLD_TAG) {
            Err(ZfsError::Other(why)) => {
                assert!(why.contains(names), "{case}: {why}");
                assert!(why.contains("release that hold by hand"), "{case}: {why}");
                assert!(why.ends_with("(read 3 times)"), "{case}: {why}");
            }
            other => panic!("{case}: {other:?}"),
        }
        let read = holds_of(&held);
        let once = [TAGGED_LIST, read.as_str()];
        assert_eq!(zfs.host().words(), once.repeat(3), "{case}");
    }

    // An output cut short, with no newline at its end.
    let mut cut = agent.clone();
    cut.pop();
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\n")
        .give(&holds_of(&["tank@skz-1"]), Ok(bytes_out(cut)));
    match cli(fake).tagged("tank", HOLD_TAG) {
        Err(ZfsError::Other(why)) => assert!(why.contains("does not end with a newline"), "{why}"),
        other => panic!("{other:?}"),
    }

    // A hold added between the list and the holds: more lines than the
    // list counted, once. The next read adds up.
    let two = [agent.clone(), hold_line("tank@skz-1", "late").into_bytes()].concat();
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\n")
        .ok(TAGGED_LIST, "tank@skz-1\t2\n")
        .give(&holds_of(&["tank@skz-1"]), Ok(bytes_out(two)));
    let mut zfs = cli(fake);
    assert_eq!(
        zfs.tagged("tank", HOLD_TAG),
        Ok(["skz-1".to_string()].into())
    );
    let read = holds_of(&["tank@skz-1"]);
    let once = [TAGGED_LIST, read.as_str()];
    assert_eq!(zfs.host().words(), once.repeat(2));
}

/// `held`, the hold's check and the release's check read the holds under
/// `pool@t` with `zfs holds -H -r`, then the count of holds of each
/// snapshot it names (`zfs list -H -p -o name,userrefs -t snapshot`, on
/// a read's limit), and check them as the sweep does: a line forged by
/// another holder's tag, or one of a snapshot that is not `@t` of a
/// dataset under the pool, is read again and then an error that names
/// the snapshot. Not an agent's hold that is not there, for `held` and
/// the hold's check, nor a hold kept, for the release's (F1-R and G-UTF
/// of #162's review; the lead's ruling on M2.6c-5e). The tags are bytes.
/// Mutants: no count read; the count not compared; a snapshot of
/// another `@t`, or of another pool that the pool's name begins, taken;
/// a snapshot named and gone before its count no reading again; the
/// holds read as UTF-8.
#[test]
fn the_holds_under_a_snapshot_are_checked_as_the_sweeps_are() {
    const BOTH: &str = r#"{"return":{"members":{"tank":"11","tank/db":"22"}}}"#;
    const WHEN: &str = "\tWed Nov 25 10:00 2026\n";
    let line = |snapshot: &str, tag: &str| format!("{snapshot}\t{tag}{WHEN}");
    let both_held = [
        line("tank@skz-1", HOLD_TAG),
        line("tank/db@skz-1", HOLD_TAG),
    ]
    .concat();
    let both = refs_words(&["tank/db@skz-1", "tank@skz-1"]);

    // Another holder's tag `caf\xe9` beside the agent's: read as bytes.
    let mut holds = both_held.clone().into_bytes();
    holds.extend_from_slice(b"tank/db@skz-1\tcaf\xe9\tWed Nov 25 10:00 2026\n");
    let mut fake = Fake::default();
    fake.give(HOLDS_R, Ok(bytes_out(holds)))
        .ok(MEMBERS_WORDS, BOTH)
        .ok(&both, "tank/db@skz-1\t2\ntank@skz-1\t1\n");
    let mut zfs = cli(fake);
    let members = zfs.members("tank", "skz-1").expect("the members");
    zfs.host().ran.clear();
    assert_eq!(
        zfs.held("tank", "skz-1", HOLD_TAG, members.as_deref()),
        Ok(Held::All)
    );
    assert_eq!(zfs.host().words(), [HOLDS_R, both.as_str()]);
    assert_eq!(zfs.host().ran[1].limit, Limits::default().read, "the count");

    // tank/db@skz-1 is held by another, whose tag forges the agent's
    // hold there: without the count, `held` reads All.
    let forged = [
        line("tank@skz-1", HOLD_TAG),
        line("tank/db@skz-1", &format!("x\tY\ntank/db@skz-1\t{HOLD_TAG}")),
    ]
    .concat();
    let odd: [(&str, String, &str, &str); 5] = [
        (
            "a line forged whole",
            forged.clone(),
            "tank/db@skz-1\t1\ntank@skz-1\t1\n",
            "`tank/db@skz-1` has 2 lines of holds and 1 holds",
        ),
        (
            "a snapshot left out of the count",
            both_held.clone(),
            "tank@skz-1\t1\n",
            "`tank/db@skz-1` has 1 lines of holds and 0 holds",
        ),
        (
            "a snapshot of another `@t`",
            [both_held.clone(), line("tank/db@skz-12", HOLD_TAG)].concat(),
            "tank/db@skz-1\t1\ntank@skz-1\t1\n",
            "\"tank/db@skz-12\\tskyzfs-publish\\tWed Nov 25 10:00 2026\"",
        ),
        (
            "a snapshot of a pool that the pool's name begins",
            [both_held.clone(), line("tankx/db@skz-1", HOLD_TAG)].concat(),
            "tank/db@skz-1\t1\ntank@skz-1\t1\n",
            "\"tankx/db@skz-1\\tskyzfs-publish\\tWed Nov 25 10:00 2026\"",
        ),
        (
            "a line of four fields",
            [
                line("tank@skz-1", HOLD_TAG),
                line("tank/db@skz-1", "skyzfs-publish\tz"),
            ]
            .concat(),
            "tank/db@skz-1\t1\ntank@skz-1\t1\n",
            "after a hold of `tank@skz-1`: \"tank/db@skz-1\\tskyzfs-publish\\tz",
        ),
    ];
    for (case, holds, refs, names) in odd {
        let mut fake = Fake::default();
        fake.ok(HOLDS_R, &holds).ok(&both, refs);
        let mut zfs = cli(fake);
        match zfs.held("tank", "skz-1", HOLD_TAG, None) {
            Err(ZfsError::Other(why)) => {
                assert!(why.contains(names), "{case}: {why}");
                assert!(why.contains(HOLDS_R), "{case}: {why}");
                assert!(why.ends_with("(read 3 times)"), "{case}: {why}");
            }
            other => panic!("{case}: {other:?}"),
        }
        let words = zfs.host().words();
        assert_eq!(
            words.iter().filter(|w| **w == HOLDS_R).count(),
            3,
            "{case}: {words:?}"
        );
    }

    // The hold's check: the forged line would say that tank/db@skz-1 has
    // the tag. The release's: that it kept the hold.
    const EXISTS: &str =
        "cannot hold snapshot 'tank/db@skz-1': tag already exists on this dataset\n";
    let mut fake = Fake::default();
    fake.fails(HOLD, 1, EXISTS)
        .ok(MEMBERS_WORDS, BOTH)
        .ok(HOLDS_R, &forged)
        .ok(&both, "tank/db@skz-1\t1\ntank@skz-1\t1\n");
    match cli(fake).hold("tank", "skz-1", HOLD_TAG) {
        Err(ZfsError::Other(why)) => assert!(
            why.contains("`tank/db@skz-1` has 2 lines of holds and 1 holds"),
            "{why}"
        ),
        other => panic!("{other:?}"),
    }
    const NO_TAG: &str =
        "cannot release hold from snapshot 'tank/db@skz-1': no such tag on this dataset\n";
    let only = line("tank/db@skz-1", &format!("x\tY\ntank/db@skz-1\t{HOLD_TAG}"));
    let mut fake = Fake::default();
    fake.fails(RELEASE, 1, NO_TAG)
        .ok(HOLDS_R, &only)
        .ok(&refs_words(&["tank/db@skz-1"]), "tank/db@skz-1\t1\n");
    match cli(fake).release("tank", "skz-1", HOLD_TAG) {
        Err(ZfsError::Other(why)) => {
            assert!(
                why.contains("`tank/db@skz-1` has 2 lines of holds and 1 holds"),
                "{why}"
            );
            assert!(!why.contains("kept the hold"), "{why}");
        }
        other => panic!("{other:?}"),
    }

    // A snapshot named and gone before its count: read again, as a
    // snapshot gone during `holds -r` is.
    let mut fake = Fake::default();
    fake.ok(HOLDS_R, &both_held)
        .fails(
            &both,
            1,
            "cannot open 'tank/db@skz-1': dataset does not exist\n",
        )
        .ok(HOLDS_R, &line("tank@skz-1", HOLD_TAG))
        .ok(&refs_words(&["tank@skz-1"]), "tank@skz-1\t1\n");
    let mut zfs = cli(fake);
    assert_eq!(zfs.held("tank", "skz-1", HOLD_TAG, None), Ok(Held::Part));
    assert_eq!(
        zfs.host().words(),
        [
            HOLDS_R,
            both.as_str(),
            HOLDS_R,
            refs_words(&["tank@skz-1"]).as_str()
        ]
    );
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
    // Nothing the channel programs' sandbox leaves out: on ZFS 2.2 and
    // 2.4 a call of `pcall` failed the program (the recordings of Q10),
    // and the sandbox has no file, operating system or loading functions
    // either (zfs-program(8)).
    for program in [MEMBERS_PROGRAM] {
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
/// scaled. The snapshots' count takes the longest limit whatever the
/// datasets (the lead's ruling on M2.6c-5d).
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
            [MEMBERS_WORDS, TAGGED_LIST, "zpool status -P tank"]
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
        assert_eq!(zfs.host().ran[1].limit, base.full_most, "{n}");
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
/// counts the snapshots (`zfs list -r -H -o name -t snapshot`, within
/// the longest limit), and the list takes [`Limits::full_per_dataset`] for
/// each dataset and [`Limits::list_per_snapshot`] (10 ms, the lead's
/// ruling on M2.6c-5d) for each snapshot, at most [`Limits::full_most`]. The
/// two gets, which read only file systems and volumes, and every other
/// command but `zfs holds -r` and the sweep's list (which grow too, by
/// their own allowance: R1-reach of #160's review) keep the datasets'
/// scale. A snapshot count that fails leaves
/// the list on the datasets' scale (the conservative reading: no longer
/// limit than before on a pool whose commands fail), and the count of
/// the datasets stands. Mutants: the snapshots not counted, counted but
/// not added, added to the gets or to the other commands, the list not
/// capped, a failed snapshot count failing the count or setting the
/// bases, the snapshot count within the count's own limit.
#[test]
fn the_lists_limit_grows_with_the_snapshots_counted() {
    let base = Limits::default();
    assert_eq!(base.list_per_snapshot, Duration::from_millis(10));
    let at = |n: u64, per_ms: u64| base.read + Duration::from_millis(per_ms * n);
    let ms = Duration::from_millis;
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
        (1, at(10, 200) + ms(10)),
        (990, at(10, 200) + ms(9_900)),
        (20_000, at(10, 200) + ms(200_000)),
        // 62 s + 10 ms × 203 700 is just below 35 minutes, and 10 ms ×
        // 203 900 past them.
        (203_700, at(10, 200) + ms(2_037_000)),
        (203_900, base.full_most),
        (1_000_000, base.full_most),
    ] {
        zfs.host().commands.remove(SNAPSHOTS);
        zfs.host().ok(SNAPSHOTS, &snapshots(m));
        zfs.host().ran.clear();
        assert_eq!(zfs.count("tank"), Ok(10), "{m}");
        assert_eq!(zfs.host().words(), [COUNT, SNAPSHOTS], "{m}");
        assert_eq!(zfs.host().ran[1].limit, base.full_most, "{m}");
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
        let holds = (at(10, 100) + base.holds_per_snapshot * u32::try_from(m).expect("small"))
            .min(base.full_most);
        assert_eq!(ran[3].limit, holds, "{m}");
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

/// R1-reach of #160's review, the lead's ruling (b): `zfs holds -r`
/// and the sweep's list of snapshots walk every snapshot in the pool
/// too, so their limits grow with the snapshots counted: a read's base
/// and 100 ms a dataset as before (at most [`Limits::most`]), plus
/// [`Limits::holds_per_snapshot`] for each snapshot (`holds -r`) or
/// [`Limits::list_per_snapshot`] (the sweep's list, as the manifest's
/// list), at most [`Limits::full_most`]. A snapshot count that fails leaves both on
/// a read's limit, as before. Mutants: either read not grown, grown by
/// the other's allowance, not capped, capped at `most`, or on the list's
/// limit after a failed count.
#[test]
fn the_holds_and_the_tags_grow_with_the_snapshots_counted() {
    let base = Limits {
        holds_per_snapshot: Duration::from_millis(30),
        ..Limits::default()
    };
    let ms = Duration::from_millis;
    let snapshots = |n: usize| -> String { (0..n).map(|i| format!("tank@s{i}\n")).collect() };
    let mut fake = Fake::default();
    fake.ok(COUNT, &listed(10));
    fake.ok(HOLDS_R, "");
    fake.ok(TAGGED_LIST, "");
    let mut zfs = ZfsCli::new(fake, base);
    let read = base.read + ms(100 * 10);
    // (snapshots counted, `holds -r`'s limit, the sweep's list's limit)
    for (m, holds, tagged) in [
        (0, read, read),
        (1, read + ms(30), read + ms(10)),
        (1000, read + ms(30_000), read + ms(10_000)),
        (20_000, read + ms(600_000), read + ms(200_000)),
        // `holds -r` past 20 minutes (`most`) and below 35 (`full_most`).
        (40_000, read + ms(1_200_000), read + ms(400_000)),
        // 61 s + 30 ms × 150 000 is past 35 minutes; the sweep's list,
        // 61 s + 10 ms × 150 000, past 20 and below 35.
        (150_000, base.full_most, read + ms(1_500_000)),
        (1_000_000, base.full_most, base.full_most),
    ] {
        zfs.host().commands.remove(SNAPSHOTS);
        zfs.host().ok(SNAPSHOTS, &snapshots(m));
        assert_eq!(zfs.count("tank"), Ok(10), "{m}");
        zfs.host().ran.clear();
        let _ = zfs.held("tank", "skz-1", HOLD_TAG, Some(&[]));
        let _ = zfs.tagged("tank", HOLD_TAG);
        let ran = &zfs.host().ran;
        let words: Vec<&str> = ran.iter().map(|r| r.words.as_str()).collect();
        assert_eq!(words, [HOLDS_R, TAGGED_LIST], "{m}");
        assert_eq!(ran[0].limit, holds, "{m}: holds -r");
        assert_eq!(ran[1].limit, tagged, "{m}: the sweep's list");
    }
    zfs.host().commands.remove(SNAPSHOTS);
    zfs.host()
        .fails(SNAPSHOTS, 1, "cannot iterate filesystems: I/O error\n");
    assert_eq!(zfs.count("tank"), Ok(10), "a failed snapshot count");
    zfs.host().ran.clear();
    let _ = zfs.held("tank", "skz-1", HOLD_TAG, Some(&[]));
    let _ = zfs.tagged("tank", HOLD_TAG);
    let ran = &zfs.host().ran;
    assert_eq!(ran[0].limit, read, "holds -r on a read's limit");
    assert_eq!(ran[1].limit, read, "the sweep's list on a read's limit");
}

/// A host on which some commands take a given time: run within a
/// shorter limit, such a command ends as the runner ends it there, in
/// `TimedOut`; otherwise the fake answers it.
struct Slow {
    fake: Fake,
    takes: std::collections::BTreeMap<&'static str, Duration>,
}

impl Host for Slow {
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> std::io::Result<crate::host::Output> {
        let words = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        match self.takes.get(words.as_str()) {
            Some(took) if limit < *took => {
                self.fake.ran.push(crate::host::fake::Ran {
                    words,
                    stdin: stdin.to_vec(),
                    limit,
                });
                Err(std::io::ErrorKind::TimedOut.into())
            }
            _ => self.fake.run(program, args, stdin, limit),
        }
    }

    fn read(&mut self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        self.fake.read(path)
    }

    fn stat(&mut self, path: &std::path::Path) -> std::io::Result<crate::host::Node> {
        self.fake.stat(path)
    }
}

/// The lead's ruling on M2.6c-5d: the snapshots' count runs within the
/// longest limit, [`Limits::full_most`], and not within a limit that
/// depends on the count it is making. On 101 datasets a read's limit is
/// 70.1 s, and testing's recording counted 101 000 snapshots in 126.2 s
/// on Ubuntu 24.04 (run 45800940382): that count is not cut off, and
/// the walks grow with it. A count that still fails, here past the
/// longest limit, leaves the walks on a read's limit (the list on the
/// gets'), and each walk that then fails names the count's failure; a
/// later count that succeeds forgets it. Mutants: the count on a
/// read's limit (cut off), or on the limit before the datasets were
/// counted; a walk's error not naming the failed count, at any of the
/// three walks; the failure not forgotten.
#[test]
fn a_count_past_the_datasets_limit_is_not_cut_off() {
    let base = Limits::default();
    let ms = Duration::from_millis;
    let read = base.read + ms(100 * 101);
    assert_eq!(read, ms(70_100));
    let recorded = ms(126_200);
    assert!(read < recorded && recorded < base.full_most);
    let snapshots: String = (0..101_000).map(|i| format!("tank@s{i}\n")).collect();
    let mut fake = Fake::default();
    fake.ok(COUNT, &listed(101));
    fake.ok(SNAPSHOTS, &snapshots);
    let gone = "cannot iterate filesystems: I/O error\n";
    fake.fails(HOLDS_R, 1, gone);
    fake.fails(TAGGED_LIST, 1, gone);
    fake.fails(LIST_WORDS, 1, gone);
    let slow = Slow {
        fake,
        takes: std::collections::BTreeMap::from([(SNAPSHOTS, recorded)]),
    };
    let mut zfs = ZfsCli::new(slow, base);
    let walks = |zfs: &mut ZfsCli<Slow>| -> Vec<(Duration, String)> {
        zfs.host().fake.ran.clear();
        let held = zfs.held("tank", "skz-1", HOLD_TAG, Some(&[])).map(|_| ());
        let tagged = zfs.tagged("tank", HOLD_TAG).map(|_| ());
        let listed = zfs.datasets("tank", "skz-1").map(|_| ());
        let ran = &zfs.host().fake.ran;
        let words: Vec<&str> = ran.iter().map(|r| r.words.as_str()).collect();
        assert_eq!(words, [HOLDS_R, TAGGED_LIST, LIST_WORDS]);
        [held, tagged, listed]
            .into_iter()
            .zip(ran.iter())
            .map(|(e, r)| match e {
                Err(ZfsError::Other(why)) => (r.limit, why),
                other => panic!("{other:?}"),
            })
            .collect()
    };

    assert_eq!(zfs.count("tank"), Ok(101));
    assert_eq!(zfs.host().fake.ran[1].words, SNAPSHOTS);
    assert_eq!(zfs.host().fake.ran[1].limit, base.full_most);
    let counted = walks(&mut zfs);
    assert_eq!(counted[0].0, read + base.holds_per_snapshot * 101_000);
    assert_eq!(counted[1].0, read + base.list_per_snapshot * 101_000);
    assert_eq!(
        counted[2].0,
        base.read + ms(200 * 101) + base.list_per_snapshot * 101_000
    );
    for (_, why) in &counted {
        assert!(!why.contains("count failed"), "{why}");
    }

    // A count that takes longer than the longest limit.
    zfs.host().takes.insert(SNAPSHOTS, base.full_most + ms(1));
    zfs.host().fake.ran.clear();
    assert_eq!(zfs.count("tank"), Ok(101), "the datasets' count stands");
    assert_eq!(zfs.host().fake.ran[1].limit, base.full_most);
    let uncounted = walks(&mut zfs);
    let limits: Vec<Duration> = uncounted.iter().map(|(limit, _)| *limit).collect();
    assert_eq!(limits, [read, read, base.read + ms(200 * 101)]);
    for (_, why) in &uncounted {
        assert!(why.contains(gone.trim_end()), "{why}");
        assert!(
            why.contains(
                "(on a limit that did not grow with the snapshots, since their count failed: "
            ),
            "{why}"
        );
        assert!(why.contains(SNAPSHOTS), "{why}");
    }

    // A missing pool or a snapshot gone stays what it is, and does not
    // name the count (Q3 of #162's review).
    for (pools, missing) in [("other\n", true), ("tank\n", false)] {
        let fake = &mut zfs.host().fake;
        fake.commands.remove(TAGGED_LIST);
        fake.commands.remove(POOLS);
        fake.fails(
            TAGGED_LIST,
            1,
            "cannot open 'tank': dataset does not exist\n",
        )
        .ok(POOLS, pools);
        let read = zfs.tagged("tank", HOLD_TAG);
        if missing {
            assert_eq!(read, Err(ZfsError::NoPool(TAGGED_LIST.into())), "{pools:?}");
        } else {
            assert!(
                matches!(&read, Err(ZfsError::Vanished(why)) if !why.contains("count failed")),
                "{pools:?}: {read:?}"
            );
        }
    }
    let fake = &mut zfs.host().fake;
    fake.commands.remove(TAGGED_LIST);
    fake.fails(TAGGED_LIST, 1, gone);

    // A dataset count that fails forgets it too: the walks are then on
    // the bases, and their errors say nothing of a count (Q4).
    let fake = &mut zfs.host().fake;
    fake.commands.remove(COUNT);
    fake.fails(COUNT, 1, gone);
    assert!(zfs.count("tank").is_err());
    for (limit, why) in walks(&mut zfs) {
        assert!(!why.contains("count failed"), "{why}");
        assert_eq!(limit, base.read, "the bases");
    }
    let fake = &mut zfs.host().fake;
    fake.commands.remove(COUNT);
    fake.ok(COUNT, &listed(101));
    zfs.host().takes.insert(SNAPSHOTS, base.full_most + ms(1));
    assert_eq!(zfs.count("tank"), Ok(101));

    // A count that succeeds again forgets the failure.
    zfs.host().takes.insert(SNAPSHOTS, recorded);
    assert_eq!(zfs.count("tank"), Ok(101));
    for (_, why) in walks(&mut zfs) {
        assert!(!why.contains("count failed"), "{why}");
    }
}

/// `zfs holds -r`'s default allowance against testing's recording of
/// it at R1's scales (run 45800940382, Ubuntu 24.04 and 26.04, idle
/// pool, one run on each image): at each scale the limit is a read's
/// on the datasets plus 10 ms a snapshot (the lead's ruling on
/// M2.6c-5d), and at least four times the seconds recorded; the
/// sweep's list at least four times the snapshots' list recorded. Mutants: the
/// allowance at another value, or not grown.
#[test]
fn the_holds_limit_covers_its_recording() {
    let base = Limits::default();
    assert_eq!(base.holds_per_snapshot, Duration::from_millis(10));
    let snapshots = |n: usize| -> String { (0..n).map(|i| format!("tank@s{i}\n")).collect() };
    // (datasets, snapshots in the pool, seconds `holds -r` took): 24.04,
    // then 26.04.
    for (datasets, m, took) in [
        (11, 11_000, 18.909),
        (101, 101_000, 174.178),
        (11, 11_000, 3.933),
        (11, 60_500, 21.903),
        (101, 101_000, 37.272),
    ] {
        let mut fake = Fake::default();
        fake.ok(COUNT, &listed(datasets));
        fake.ok(SNAPSHOTS, &snapshots(m));
        fake.ok(HOLDS_R, "");
        let mut zfs = ZfsCli::new(fake, base);
        assert_eq!(zfs.count("tank"), Ok(datasets as u64), "{m}");
        zfs.host().ran.clear();
        let _ = zfs.held("tank", "skz-1", HOLD_TAG, Some(&[]));
        let limit = zfs.host().ran[0].limit;
        let read = base.read + base.per_dataset * u32::try_from(datasets).expect("small");
        let grown = Duration::from_millis(10 * u64::try_from(m).expect("small"));
        assert_eq!(limit, read + grown, "{m}");
        assert!(limit.as_secs_f64() >= 4.0 * took, "{m}: {limit:?}");
    }
    // The sweep's list against the snapshots' list recorded on 24.04,
    // the slower image: (datasets, snapshots, seconds).
    for (datasets, m, took) in [(11, 11_000, 20.3), (101, 101_000, 185.9)] {
        let mut fake = Fake::default();
        fake.ok(COUNT, &listed(datasets));
        fake.ok(SNAPSHOTS, &snapshots(m));
        fake.ok(TAGGED_LIST, "");
        let mut zfs = ZfsCli::new(fake, base);
        assert_eq!(zfs.count("tank"), Ok(datasets as u64), "{m}");
        zfs.host().ran.clear();
        assert_eq!(zfs.tagged("tank", HOLD_TAG), Ok(BTreeSet::new()), "{m}");
        let limit = zfs.host().ran[0].limit;
        assert!(limit.as_secs_f64() >= 4.0 * took, "{m}: {limit:?}");
    }
}

/// What libzfs says of a snapshot gone between a command's open of it
/// and its read, in the two shapes `no such pool` takes. The first is
/// testing's agent rig's; the second, a channel program's, is not known
/// to happen: a read-only program holds the pool's configuration lock
/// for its whole run, so a destroy, a sync task, cannot come in the
/// middle of it (nit B of #161's review). The reading of `members` again
/// is defensive (#160's review).
const VANISHED: [&str; 2] = [
    "cannot get holds for 'tank/db@skz-1': no such pool or dataset\n",
    "Channel program execution failed: no such pool\n",
];

/// A read that says `no such pool` while `zpool list` lists the pool is
/// a dataset gone during the read, a destroy's (testing's agent rig,
/// case `destroy`; the lead's ask): the members, the holds and the tags
/// are read again, and the second read is what comes back. Mutants: any
/// `no such pool` read as a missing pool without `zpool list`, a read
/// not made again.
#[test]
fn a_read_whose_snapshot_vanished_is_read_again() {
    for stderr in VANISHED {
        let mut fake = Fake::default();
        fake.fails(MEMBERS_WORDS, 1, stderr)
            .ok(MEMBERS_WORDS, r#"{"return":{"absent":"1"}}"#)
            .ok(POOLS, "tank\n");
        let mut zfs = cli(fake);
        assert_eq!(zfs.members("tank", "skz-1"), Ok(None), "{stderr}");
        assert_eq!(
            zfs.host().words(),
            [MEMBERS_WORDS, POOLS, MEMBERS_WORDS],
            "{stderr}"
        );

        let mut fake = Fake::default();
        fake.fails(HOLDS_R, 1, stderr)
            // What `zfs holds -r` of a snapshot that is gone gives: exit
            // 0, no line, and `no datasets available` (fidelity, #160's
            // review), as read in OpenZFS 2.2's `zfs_do_holds` and not
            // recorded (nit F of #161's review).
            .give(
                HOLDS_R,
                Ok(crate::host::fake::output(0, "", "no datasets available\n")),
            )
            .ok(POOLS, "other\ntank\n");
        let mut zfs = cli(fake);
        assert_eq!(
            zfs.held("tank", "skz-1", HOLD_TAG, None),
            Ok(Held::No),
            "{stderr}"
        );
        assert_eq!(zfs.host().words(), [HOLDS_R, POOLS, HOLDS_R], "{stderr}");
    }
}

/// A read that goes on vanishing is made [`VANISHED_READS`] times in
/// all, then fails with an error that names the command and what it
/// said: not a missing pool. Mutants: no bound, one read more or less.
#[test]
fn a_read_that_goes_on_vanishing_is_an_error_that_names_it() {
    assert_eq!(VANISHED_READS, 3);
    let mut fake = Fake::default();
    fake.fails(HOLDS_R, 1, VANISHED[0]).ok(POOLS, "tank\n");
    let mut zfs = cli(fake);
    let read = zfs.held("tank", "skz-1", HOLD_TAG, None);
    // What the operator reads: the command, its message, and why it is
    // not a missing pool (#160's review).
    assert_eq!(
        read.map_err(|e| e.to_string()),
        Err(format!(
            "{HOLDS_R}: {} (`zpool list` lists its pool)",
            VANISHED[0].trim_end()
        ))
    );
    assert_eq!(
        zfs.host().words(),
        [HOLDS_R, POOLS, HOLDS_R, POOLS, HOLDS_R, POOLS]
    );
    // Commands that are not read again: one try.
    let mut fake = Fake::default();
    fake.fails(LIST_WORDS, 1, VANISHED[1]).ok(POOLS, "tank\n");
    let mut zfs = cli(fake);
    assert!(matches!(
        zfs.datasets("tank", "skz-1"),
        Err(ZfsError::Vanished(_))
    ));
    assert_eq!(zfs.host().words(), [LIST_WORDS, POOLS]);
}

/// `no such pool` is a missing pool only where `zpool list` does not
/// list the pool the command acts on: a pool whose name only begins with the
/// command's is not it, nor is one whose name the command's only begins
/// with. Where `zpool list` fails, an error that says both, and not a
/// missing pool. A missing pool's message names the command. Mutants: a
/// name matched as a prefix, `zpool list`'s failure read as no pool.
#[test]
fn no_such_pool_is_a_missing_pool_only_where_zpool_list_agrees() {
    let said = "cannot open 'tank': no such pool\n";
    for (listed, gone) in [
        ("", true),
        ("tank2\nta\nother\n", true),
        ("tank\n", false),
        ("tank2\ntank\n", false),
    ] {
        let mut fake = Fake::default();
        fake.fails(MEMBERS_WORDS, 1, said).ok(POOLS, listed);
        let read = cli(fake).members("tank", "skz-1");
        if gone {
            assert_eq!(
                read,
                Err(ZfsError::NoPool(MEMBERS_WORDS.into())),
                "{listed:?}"
            );
        } else {
            assert!(matches!(read, Err(ZfsError::Vanished(_))), "{listed:?}");
        }
    }
    for list in [
        Ok(crate::host::fake::output(
            1,
            "",
            "internal error: out of memory\n",
        )),
        Err(std::io::ErrorKind::TimedOut),
    ] {
        let mut fake = Fake::default();
        fake.fails(MEMBERS_WORDS, 1, said).give(POOLS, list.clone());
        let read = cli(fake).members("tank", "skz-1");
        assert!(
            matches!(&read, Err(ZfsError::Other(why))
                if why.contains(MEMBERS_WORDS) && why.contains("no such pool")),
            "{list:?}: {read:?}"
        );
    }
    let message = ZfsError::NoPool("zpool status -P tank".into()).to_string();
    assert_eq!(message, "zpool status -P tank: no such pool");
    for (name, pool) in [
        ("tank", "tank"),
        ("tank@t", "tank"),
        ("tank/db@t", "tank"),
        ("tank/a@b/c", "tank"),
        ("tank2/db", "tank2"),
    ] {
        assert_eq!(pool_of(name), pool, "{name}");
    }
}

/// Only the pool a command acts on is looked for in `zpool list`, not
/// every word of the command: a pool named like a subcommand, an
/// option's value or the take's name is another pool (Codex on #160).
/// A value read alone acts on its dataset's pool; the layout's `zpool
/// status`, alone or after `zpool get`'s `missing pool name`, and the
/// assist's `zpool sync` on theirs (TG2 of #161's review). Mutants: any word matched, a value's
/// dataset taken for its pool.
#[test]
fn only_the_commands_own_pool_is_looked_for() {
    let said = "cannot open 'tank': no such pool\n";
    let others = "get\nproperty\nstatus\nskz-1\nskyzfs-publish\nprogram\nholds\n";
    let props =
        "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint tank";
    let mut fake = Fake::default();
    fake.fails(props, 1, said).ok(POOLS, others);
    assert_eq!(cli(fake).props("tank"), Err(ZfsError::NoPool(props.into())));
    let mut fake = Fake::default();
    fake.fails(MEMBERS_WORDS, 1, said).ok(POOLS, others);
    assert_eq!(
        cli(fake).members("tank", "skz-1"),
        Err(ZfsError::NoPool(MEMBERS_WORDS.into()))
    );
    let mut fake = Fake::default();
    fake.fails(TAGGED_LIST, 1, said).ok(POOLS, others);
    assert_eq!(
        cli(fake).tagged("tank", "skyzfs-publish"),
        Err(ZfsError::NoPool(TAGGED_LIST.into()))
    );
    // `zfs list` says `dataset does not exist` of a missing pool.
    let mut fake = Fake::default();
    fake.fails(
        TAGGED_LIST,
        1,
        "cannot open 'tank': dataset does not exist\n",
    )
    .ok(POOLS, others);
    assert_eq!(
        cli(fake).tagged("tank", "skyzfs-publish"),
        Err(ZfsError::NoPool(TAGGED_LIST.into()))
    );
    // The layout's `zpool status`, alone and after `zpool get`'s
    // `missing pool name` (ZFS 2.2), with a pool named `status` listed
    // (TG2 of #161's review).
    let status = "zpool status -P tank";
    let mut fake = Fake::default();
    fake.fails(status, 1, said).ok(POOLS, others);
    assert_eq!(
        cli(fake).layout("tank"),
        Err(ZfsError::NoPool(status.into()))
    );
    // The second, `-g`, after a first that read.
    let guided = "zpool status -P -g tank";
    let mut fake = Fake::default();
    fake.ok(status, RIG)
        .fails(guided, 1, said)
        .ok(POOLS, others);
    assert_eq!(
        cli(fake).layout("tank"),
        Err(ZfsError::NoPool(guided.into()))
    );
    let mut fake = Fake::default();
    fake.fails(props, 2, "missing pool name\n")
        .fails(status, 1, said)
        .ok(POOLS, others);
    assert_eq!(
        cli(fake).props("tank"),
        Err(ZfsError::NoPool(status.into()))
    );
    // The assist's `zpool sync`, with a pool named `sync` listed.
    let mut fake = Fake::default();
    fake.ok("zfs set skyzfs:assist=1 tank", "")
        .fails("zpool sync tank", 1, said)
        .ok(POOLS, "sync\nskyzfs:assist=1\n");
    assert_eq!(
        cli(fake).assist("tank", 1),
        Err(ZfsError::NoPool("zpool sync tank".into()))
    );
    // The sweep's `zfs holds -H`, with pools named after its words and
    // its snapshots listed.
    let sweep_holds = holds_of(&["tank@skz-1"]);
    let mut fake = Fake::default();
    fake.ok(TAGGED_LIST, "tank@skz-1\t1\n")
        .fails(
            &sweep_holds,
            1,
            "cannot get holds for 'tank@skz-1': no such pool or dataset\n",
        )
        .ok(POOLS, "tank@skz-1\nholds\n-H\n");
    assert_eq!(
        cli(fake).tagged("tank", "skyzfs-publish"),
        Err(ZfsError::NoPool(sweep_holds.clone()))
    );
    let mut fake = Fake::default();
    fake.fails(
        HOLDS_R,
        1,
        "cannot open 'tank@skz-1': no such pool or dataset\n",
    )
    .ok(POOLS, "skz-1\nholds\n");
    assert_eq!(
        cli(fake).held("tank", "skz-1", "skyzfs-publish", None),
        Err(ZfsError::NoPool(HOLDS_R.into()))
    );

    // A value read alone acts on the pool of its dataset: `tank`, listed,
    // so the dataset vanished, which is no missing pool.
    let pairs = "tank/db\ta:b\n";
    let values = "tank/db\ta:b\tc\nd\n";
    let mut fake = manifest_reads(pairs, values.as_bytes());
    fake.fails(
        &alone("a:b", "tank/db"),
        1,
        "cannot open 'tank/db': no such pool or dataset\n",
    )
    .ok(POOLS, "tank\n");
    let read = cli(fake).datasets("tank", "skz-1");
    assert!(
        matches!(&read, Err(ZfsError::Vanished(why)) if why.contains(&alone("a:b", "tank/db"))),
        "{read:?}"
    );
}
