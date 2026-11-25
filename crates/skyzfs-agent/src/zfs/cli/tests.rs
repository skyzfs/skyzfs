//! The adapter on a fake host: the commands it runs, and its parsers on
//! output written by hand after OpenZFS's `zpool status`, `zpool get`
//! and `zfs program -j`. The parsers on output recorded from ZFS are
//! in `recordings.rs`.

use std::path::PathBuf;

use super::*;
use crate::host::fake::Fake;
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

const SNAPSHOT_WORDS: &str =
    "zfs program -j -t 100000000 -m 104857600 tank - tank skz-1 skyzfs:assist";

const TAKEN: &str = r#"{"return":{"createtxg":"812","members":{"tank/vol":{"guid":"-2","local":{"volsize":"1073741824"},"snapshot_guid":"33","type":"volume"},"tank":{"guid":"11","local":{"compression":"lz4","skyzfs:assist":"7"},"snapshot_guid":"12","type":"filesystem"},"tank/db":{"guid":"21","local":[],"snapshot_guid":"22","type":"filesystem"}}}}"#;

/// The snapshot program is run on its standard input with the pool, the
/// name and the agent's property, within the limit of a change; what it
/// returns makes the manifest's members, root first and then by name,
/// a GUID above 2^63 read from its signed form, and the agent's own
/// property left out. Mutants: the order, the sign, the property kept,
/// the program not given.
#[test]
fn the_snapshot_program_makes_the_manifests_members() {
    let mut fake = Fake::default();
    fake.ok(SNAPSHOT_WORDS, TAKEN);
    let mut zfs = cli(fake);
    let program = zfs.snapshot("tank", "skz-1").expect("taken");
    let local = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>()
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
    let ran = &zfs.host().ran;
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].stdin, SNAPSHOT_PROGRAM.as_bytes());
    assert_eq!(ran[0].limit, Limits::default().change);
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

/// A member's `@t` that exists, found before any is taken, is
/// [`ZfsError::Exists`]; any other failure of the program is an error
/// that says whether anything was taken. Mutants: `EEXIST` after a
/// partial take read as nothing taken, a property's failure lost.
#[test]
fn the_snapshot_programs_failures_say_what_was_taken() {
    let run = |returned: &str| {
        let mut fake = Fake::default();
        fake.ok(SNAPSHOT_WORDS, &format!(r#"{{"return":{returned}}}"#));
        cli(fake).snapshot("tank", "skz-1")
    };
    assert_eq!(
        run(r#"{"failed":"tank/db@skz-1","errno":"17"}"#),
        Err(ZfsError::Exists)
    );
    let Err(ZfsError::Other(why)) = run(r#"{"failed":"tank/db@skz-1","errno":"28"}"#) else {
        panic!("not an error");
    };
    assert!(why.contains("errno 28, before any was taken"), "{why}");
    let Err(ZfsError::Other(why)) = run(r#"{"failed":"tank/db@skz-1","errno":"17","taken":"1"}"#)
    else {
        panic!("not an error");
    };
    assert!(why.contains("after 1 were taken"), "{why}");
    // A failure with no errno is an error too.
    assert!(matches!(
        run(r#"{"failed":"tank/db"}"#),
        Err(ZfsError::Other(why)) if why.contains("tank/db")
    ));
    let mut fake = Fake::default();
    fake.fails(
        SNAPSHOT_WORDS,
        1,
        "Channel program execution failed:\n[string \"channel program\"]:9: oops\n",
    );
    assert!(matches!(
        cli(fake).snapshot("tank", "skz-1"),
        Err(ZfsError::Other(why)) if why.contains("oops")
    ));
    let mut fake = Fake::default();
    fake.fails(SNAPSHOT_WORDS, 1, "cannot open 'tank': no such pool\n");
    assert_eq!(cli(fake).snapshot("tank", "skz-1"), Err(ZfsError::NoPool));
}

const DESTROY: &str = "zfs destroy -r tank@skz-1";

/// The snapshot program's output given as bytes, exit 0, and what
/// `zfs destroy -r tank@skz-1` then gives.
fn taken_unread(
    stdout: &[u8],
    destroy: Result<&str, &str>,
) -> (Result<Program, ZfsError>, Vec<String>) {
    let mut fake = Fake::default();
    fake.give(
        SNAPSHOT_WORDS,
        Ok(Output {
            code: Some(0),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }),
    );
    match destroy {
        Ok(out) => fake.ok(DESTROY, out),
        Err(e) => fake.fails(DESTROY, 1, e),
    };
    let mut zfs = cli(fake);
    let done = zfs.snapshot("tank", "skz-1");
    let words = zfs.host().words().into_iter().map(String::from).collect();
    (done, words)
}

/// Output of the snapshot program that cannot be read after it exited
/// 0 (cut off, as `zfs program -j` cuts it at a byte it cannot print
/// and still exits 0; not UTF-8; a result of the wrong shape that does
/// not say it failed) is an error, and `pool@t` is destroyed with what
/// it took before anything relies on it; the error says what became of
/// it. Mutants: a cut-off output taken as a result, the destroy left
/// out, run on a failure the program reported, or its failure lost.
#[test]
fn output_of_the_snapshot_program_that_cannot_be_read_destroys_what_it_took() {
    let cut = &TAKEN.as_bytes()[..TAKEN.len() / 2];
    let shapes: [&[u8]; 6] = [
        cut,
        b"",
        b"{\"return\":{\"createtxg\":\"812\",\"members\":{\"tank\":\xff}}}",
        br#"{"return":{"createtxg":"x","members":{}}}"#,
        br#"{"return":{"createtxg":"1","members":{"tank/db":{"guid":"1","local":{},"snapshot_guid":"2","type":"filesystem"}}}}"#,
        br#"{"return":{"createtxg":"1"}}"#,
    ];
    for stdout in shapes {
        let shown = String::from_utf8_lossy(stdout);
        let (done, words) = taken_unread(stdout, Ok(""));
        assert_eq!(words, [SNAPSHOT_WORDS, DESTROY], "{shown}");
        let Err(ZfsError::Other(why)) = done else {
            panic!("{shown}: {done:?}");
        };
        assert!(why.contains("tank@skz-1 destroyed"), "{shown}: {why}");
        // The program is named once (NL-S4).
        assert_eq!(why.matches("zfs program").count(), 1, "{shown}: {why}");
    }
    for (gone, became) in [
        (
            "could not find any snapshots to destroy; check snapshot names.\n",
            "tank@skz-1 was not taken",
        ),
        (
            "cannot open 'tank': dataset does not exist\n",
            "tank@skz-1 was not found",
        ),
    ] {
        let (done, _) = taken_unread(cut, Err(gone));
        assert!(
            matches!(&done, Err(ZfsError::Other(why))
                if why.contains("cannot be read") && why.contains(became)),
            "{gone}: {done:?}"
        );
    }
    let (done, _) = taken_unread(
        cut,
        Err("cannot destroy snapshot tank@skz-1: pool I/O is currently suspended\n"),
    );
    assert!(
        matches!(&done, Err(ZfsError::Other(why)) if why.contains("could not be destroyed")),
        "{done:?}"
    );
    // A failure the program reported before it took any is not
    // destroyed (one after it took some is:
    // `a_take_that_failed_part_way_is_destroyed`).
    let (done, words) = taken_unread(
        br#"{"return":{"failed":"tank/db@skz-1","errno":"28"}}"#,
        Ok(""),
    );
    assert_eq!(words, [SNAPSHOT_WORDS]);
    assert!(matches!(done, Err(ZfsError::Other(_))));
}

/// A take the program says failed after it took some members' `@t`, on
/// any errno but `EEXIST`, is destroyed, and the error says what was
/// taken and what became of it (the lead's decision on testing's scale
/// run: 10000 file systems, `ENOSPC` after 4563). Nothing taken, or
/// `EEXIST`, destroys nothing. Mutants: the destroy left out; run with
/// nothing taken; run after `EEXIST`; its failure lost.
#[test]
fn a_take_that_failed_part_way_is_destroyed() {
    // ENOSPC, EDQUOT (Linux), EIO; the first as the scale run recorded it.
    for (errno, stdout) in [
        (
            "28",
            r#"{"return":{"errno":"28","taken":"4563","failed":"skzqs/d5199@t"}}"#,
        ),
        (
            "122",
            r#"{"return":{"failed":"tank/db@skz-1","errno":"122","taken":"2"}}"#,
        ),
        (
            "5",
            r#"{"return":{"failed":"tank/db@skz-1","errno":"5","taken":"1"}}"#,
        ),
    ] {
        let (done, words) = taken_unread(stdout.as_bytes(), Ok(""));
        assert_eq!(words, [SNAPSHOT_WORDS, DESTROY], "{errno}");
        let Err(ZfsError::Other(why)) = done else {
            panic!("{errno}: {done:?}");
        };
        assert!(why.contains(&format!("errno {errno}, after ")), "{why}");
        assert!(why.ends_with("; tank@skz-1 destroyed"), "{why}");
        assert_eq!(why.matches("zfs program").count(), 1, "{why}");
    }
    // The destroy itself fails: the error says so, and what was taken.
    let (done, words) = taken_unread(
        br#"{"return":{"failed":"tank/db@skz-1","errno":"28","taken":"3"}}"#,
        Err("cannot destroy snapshot tank@skz-1: pool I/O is currently suspended\n"),
    );
    assert_eq!(words, [SNAPSHOT_WORDS, DESTROY]);
    assert!(
        matches!(&done, Err(ZfsError::Other(why))
            if why.contains("after 3 were taken") && why.contains("tank@skz-1 could not be destroyed")),
        "{done:?}"
    );
    // Nothing taken, or a name that exists: nothing destroyed.
    for stdout in [
        r#"{"return":{"failed":"tank/db@skz-1","errno":"28","taken":"0"}}"#,
        r#"{"return":{"failed":"tank/db@skz-1","errno":"28"}}"#,
        r#"{"return":{"failed":"tank/db@skz-1","errno":"17","taken":"1"}}"#,
    ] {
        let (done, words) = taken_unread(stdout.as_bytes(), Ok(""));
        assert_eq!(words, [SNAPSHOT_WORDS], "{stdout}");
        assert!(
            matches!(done, Err(ZfsError::Other(_))),
            "{stdout}: {done:?}"
        );
    }
}

/// `zfs destroy` reads `%` ranges and `,` lists after the `@`: a name
/// that is not a snapshot's name is not destroyed (NL-S1). Mutant: the
/// guard left out.
#[test]
fn what_is_not_a_snapshots_name_is_not_destroyed() {
    for t in ["a%z", "a,b", ""] {
        let mut zfs = cli(Fake::default());
        let ZfsError::Other(why) = zfs.destroy_taken("tank", t, "cut off") else {
            panic!("{t}");
        };
        assert!(why.contains("was not destroyed"), "{t}: {why}");
        assert!(zfs.host().words().is_empty(), "{t}");
    }
}

/// A local property's value comes as it is when it is printable ASCII
/// and in hex under `encoding` otherwise; a byte above 0x7f and bytes
/// that are not UTF-8 reach the manifest as they were. A string that is
/// not printable ASCII, a hex value of the wrong case, length or form,
/// and an unknown encoding are refused, and `pool@t` destroyed.
/// Mutants: hex not decoded, a non-ASCII string let through, an extra
/// key let through.
#[test]
fn a_value_that_is_not_printable_ascii_comes_in_hex() {
    let with = |local: &str| {
        format!(
            r#"{{"return":{{"createtxg":"812","members":{{"tank":{{"guid":"11","local":{local},"snapshot_guid":"12","type":"filesystem"}}}}}}}}"#
        )
    };
    let mut fake = Fake::default();
    fake.ok(
        SNAPSHOT_WORDS,
        &with(
            r#"{"a:latin1":{"encoding":"hex","value":"e9"},"a:bad":{"encoding":"hex","value":"ff00fe"},"a:tab":{"encoding":"hex","value":"0961"},"a:plain":"x y~","a:del":{"encoding":"hex","value":"7f"},"a:us":{"encoding":"hex","value":"1f"},"a:space":" "}"#,
        ),
    );
    let program = cli(fake).snapshot("tank", "skz-1").expect("taken");
    let local = &program.members[0].local;
    assert_eq!(local["a:latin1"], [0xe9]);
    assert_eq!(local["a:bad"], [0xff, 0x00, 0xfe]);
    assert_eq!(local["a:tab"], b"\ta");
    assert_eq!(local["a:plain"], b"x y~");
    // The edges of printable, where the program and the adapter must
    // agree (NL-F3): 0x1f and 0x7f in hex, 0x20 as it is.
    assert_eq!(local["a:del"], [0x7f]);
    assert_eq!(local["a:us"], [0x1f]);
    assert_eq!(local["a:space"], b" ");
    for bad in [
        r#"{"a":"caf\u00e9"}"#,
        r#"{"a":"a\tb"}"#,
        r#"{"a":{"encoding":"hex","value":"E9"}}"#,
        r#"{"a":{"encoding":"hex","value":"e"}}"#,
        r#"{"a":{"encoding":"hex","value":"+e"}}"#,
        r#"{"a":{"encoding":"hex","value":"61"}}"#,
        r#"{"a":{"encoding":"hex","value":"20"}}"#,
        r#"{"a":{"encoding":"hex","value":"7e"}}"#,
        r#"{"a":"\u007f"}"#,
        r#"{"a":"\u001f"}"#,
        r#"{"a":{"encoding":"hex","value":"zz"}}"#,
        r#"{"a":{"encoding":"base64","value":"6Q=="}}"#,
        r#"{"a":{"encoding":"hex","value":"e9","x":"1"}}"#,
        r#"{"a":["e9"]}"#,
    ] {
        let (done, words) = taken_unread(with(bad).as_bytes(), Ok(""));
        assert!(matches!(done, Err(ZfsError::Other(_))), "{bad}: {done:?}");
        assert_eq!(words, [SNAPSHOT_WORDS, DESTROY], "{bad}");
    }
}

/// The program and the manifest decide what is printable in the same
/// place (NL-F3): the program's pattern is `[^\32-\126]`, and
/// `manifest::plain` takes 0x20 to 0x7e. Mutant: `plain` widened to 0x7f.
#[test]
fn the_program_and_the_manifest_agree_on_what_is_printable() {
    assert!(SNAPSHOT_PROGRAM.contains(r#"string.find(v, "[^\32-\126]")"#));
    for b in 0..=u8::MAX {
        assert_eq!(manifest::plain(&[b]), (32..=126).contains(&b), "{b:#04x}");
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
        let held = zfs.held("tank", "skz-1", HOLD_TAG);
        let words: Vec<String> = zfs.host().words().iter().map(|w| w.to_string()).collect();
        (held, words)
    };
    let every = format!("{ROOT}{DB}");
    assert_eq!(
        held(Ok(&every), Some(BOTH)),
        (
            Ok(Held::All),
            vec![HOLDS_R.to_string(), MEMBERS_WORDS.to_string()]
        )
    );
    assert_eq!(
        held(Ok(ROOT), Some(BOTH)).0,
        Ok(Held::Part),
        "the root's only: tank/db@skz-1 lacks it"
    );
    assert_eq!(
        held(Ok(DB), None),
        (Ok(Held::Part), vec![HOLDS_R.to_string()]),
        "the root lacks it: a part, with no member read"
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
        "gone between the two reads: the tags read are a part"
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

/// The snapshot program checks every member and reads every property
/// before it takes the first snapshot, so that a failure takes nothing;
/// it leaves the agent's property out; and all it returns is strings.
/// Read in its text: the program itself runs on the rigs (Q10).
#[test]
fn the_snapshot_program_takes_nothing_before_it_has_read_everything() {
    let at = |needle: &str| {
        SNAPSHOT_PROGRAM
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` is not in the program"))
    };
    let take = at("zfs.sync.snapshot");
    assert!(at("zfs.check.snapshot") < take);
    assert!(at("zfs.list.system_properties") < take);
    assert!(at("user_properties(ds)") < take);
    assert!(at("property ~= assist") < take);
    assert_eq!(SNAPSHOT_PROGRAM.matches("zfs.sync.").count(), 1);
    assert!(!MEMBERS_PROGRAM.contains("zfs.sync."));
    assert!(!TAGGED_PROGRAM.contains("zfs.sync."));
    // Nothing the channel programs' sandbox leaves out: on ZFS 2.2 and
    // 2.4 a call of `pcall` failed the program (the recordings of Q10),
    // and the sandbox has no file, operating system or loading functions
    // either (zfs-program(8)).
    for program in [SNAPSHOT_PROGRAM, MEMBERS_PROGRAM, TAGGED_PROGRAM] {
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
