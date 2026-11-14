//! The uberblock parser and the count of cuts held against pools
//! recorded on real ZFS (M2.T1's fixtures, `tests/fixtures/pools/` at
//! the repository's root; their format is in that directory's README).
//!
//! A fixture is the record of every request a pool's device was handed,
//! in the order they completed (after the device applied them), with the
//! bytes of every write that touches a label, and the reference's reading
//! of it (`tests/kernel/cut_ref.py`, which parses the on-disk format
//! itself): every uberblock in those writes, and the cuts by design
//! §5.2. Here the same record goes through this crate's parser and
//! through the store's rule, one request at a time, and both must agree
//! with the reference in full. Each cut must also say whether it opens
//! as itself exactly where the fixture's check (`check.tsv`, from
//! `zdb -e -u` at each cut's instant) found the volume opening at the
//! cut's own group (work package M2.1b).
//!
//! What the recordings do not show: none of the count's defects changes
//! what these five pools make (`life`, `rewind`, `config`, `stray`,
//! `autotrim`; probed on 2026-11-14). No flush in them falls between
//! one group's uberblock writes, so a repeat never follows a cut and
//! counting repeats makes no other cut; no uberblock of group 0 is in a
//! record; and the rewind's group used twice (51) never meets its
//! earlier self as the mark or the newest cut, so a repeat told by the
//! group alone makes the same cuts. The store's own tests are what
//! notice those defects (`tests/cuts.rs`). The `multihost` recording,
//! not in yet, is the one whose stale writes are repeats after a cut.
//! Of the two defects of whether a cut opens as itself, the recordings
//! notice the rings not read at a start, and not the group alone for
//! the order (see the last test).
//!
//! The order the store is given is the record's: a write is received
//! and applied before the next record, a flush is received and reaches
//! its barrier. A write that flushes before its data (`write+flush`,
//! REQ_PREFLUSH) is a flush and then the write; a write with FUA is the
//! write and then the flush the daemon's pump adds after it, which takes
//! a mark as any flush does (the lead's decision, 2026-11-14).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use skyzfs_uberblock::{ByteOrder, Labels};

use super::*;
use crate::inplace::cuts::CutRule;

/// Where the recorded pools are.
const POOLS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/pools");

/// The version of the fixtures' format this test reads.
const FORMAT: &str = "# skyzfs cut fixture, version 1";

/// One request of a record.
#[derive(Debug)]
struct Record {
    i: u64,
    op: String,
    offset: u64,
    bytes: u64,
    /// The write's bytes, for a write that touches a label.
    data: Option<Vec<u8>>,
    /// A mark's text.
    text: String,
}

/// A recorded pool.
struct Pool {
    name: String,
    dir: PathBuf,
    dev_bytes: u64,
    records: Vec<Record>,
}

/// The recorded pools, by name. There must be some: a test that finds
/// none would pass on nothing.
fn pools() -> Vec<Pool> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(POOLS)
        .unwrap_or_else(|e| panic!("the recorded pools at {POOLS}: {e}"))
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no recorded pool under {POOLS}");
    dirs.into_iter().map(|dir| pool(&dir)).collect()
}

fn pool(dir: &Path) -> Pool {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .expect("a name")
        .to_string();
    let mut text = String::new();
    flate2::read::GzDecoder::new(
        std::fs::File::open(dir.join("requests.tsv.gz")).expect("requests.tsv.gz"),
    )
    .read_to_string(&mut text)
    .unwrap_or_else(|e| panic!("{name}: requests.tsv.gz: {e}"));
    assert_eq!(text.lines().next(), Some(FORMAT), "{name}: the format");
    let dev_bytes = header(&text, "dev_bytes").unwrap_or_else(|| panic!("{name}: no dev_bytes"));
    let records = rows(&text)
        .into_iter()
        .map(|columns| {
            let [i, op, offset, bytes, data] = columns.as_slice() else {
                panic!("{name}: a record of {} columns: {columns:?}", columns.len());
            };
            let number = |text: &str| -> u64 {
                text.parse()
                    .unwrap_or_else(|_| panic!("{name}: not a number: {text}"))
            };
            let text = data.clone();
            let data = (data != "-" && op.starts_with("write")).then(|| unhex(data));
            Record {
                i: number(i),
                op: op.clone(),
                offset: number(offset),
                bytes: number(bytes),
                data,
                text,
            }
        })
        .collect();
    Pool {
        name,
        dir: dir.to_path_buf(),
        dev_bytes,
        records,
    }
}

/// The value of a `# key N` line.
fn header(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.strip_prefix("# "))
        .find_map(|line| line.strip_prefix(key)?.trim().parse().ok())
}

/// The lines that are not comments, split at tabs.
fn rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "hex of odd length");
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

/// A root block pointer as the fixtures write it: sixteen words of
/// sixteen hex digits each.
fn root_hex(root: &[u64; 16]) -> String {
    root.iter().map(|word| format!("{word:016x}")).collect()
}

/// Every uberblock the parser finds in a recorded pool's writes, in the
/// columns of `uberblocks.tsv` that it reads: record, label, slot, byte
/// order, group, root block pointer. Every uberblock, group 0 included,
/// as the reference lists them.
#[test]
fn the_parser_finds_every_uberblock_the_reference_finds() {
    for pool in pools() {
        let labels = Labels::for_size(pool.dev_bytes);
        let mut found = Vec::new();
        for record in &pool.records {
            let Some(data) = &record.data else {
                // The record has the bytes of every write that touches
                // a label, so a write without them reaches no ring.
                assert!(
                    !record.op.starts_with("write")
                        || !labels.reaches_a_ring(record.offset, record.bytes),
                    "{}: record {} reaches a ring and has no bytes",
                    pool.name,
                    record.i
                );
                continue;
            };
            assert_eq!(data.len() as u64, record.bytes, "{}", pool.name);
            for ub in labels.uberblocks_in(record.offset, data) {
                let order = match ub.order {
                    ByteOrder::Little => "le",
                    ByteOrder::Big => "be",
                };
                found.push(vec![
                    record.i.to_string(),
                    ub.label.to_string(),
                    ub.at.to_string(),
                    order.to_string(),
                    ub.state.txg.to_string(),
                    root_hex(&ub.state.root),
                ]);
            }
        }
        let text = std::fs::read_to_string(pool.dir.join("uberblocks.tsv")).expect("uberblocks");
        assert_eq!(
            text.lines().next(),
            Some(FORMAT),
            "{}: the format",
            pool.name
        );
        // record label slot order txg mmp taken rootbp: all but mmp and
        // taken, which the parser does not say.
        let wanted: Vec<Vec<String>> = rows(&text)
            .into_iter()
            .map(|c| {
                vec![
                    c[0].clone(),
                    c[1].clone(),
                    c[2].clone(),
                    c[3].clone(),
                    c[4].clone(),
                    c[7].clone(),
                ]
            })
            .collect();
        assert!(!wanted.is_empty(), "{}: no uberblock", pool.name);
        assert_eq!(found.len(), wanted.len(), "{}: how many", pool.name);
        for (got, want) in found.iter().zip(&wanted) {
            assert_eq!(got, want, "{}", pool.name);
        }
    }
}

/// Every uberblock ZFS wrote into the recorded pools carries a label
/// checksum that holds, at the slot's own size (work package M2.1c):
/// the checksum as this crate computes it is ZFS's. None of them is of
/// group 0 (`vdev_label_init`'s, whose ring is written as one I/O).
#[test]
fn every_recorded_uberblock_is_verified_by_its_label_checksum() {
    for pool in pools() {
        let labels = Labels::for_size(pool.dev_bytes);
        let mut found = 0;
        for record in &pool.records {
            let Some(data) = &record.data else { continue };
            for ub in labels.uberblocks_in(record.offset, data) {
                assert!(
                    ub.counts() && ub.verified(),
                    "{}: record {}, label {}, slot at {}: {:?}",
                    pool.name,
                    record.i,
                    ub.label,
                    ub.at,
                    ub.slot
                );
                found += 1;
            }
        }
        assert!(found > 0, "{}: no uberblock", pool.name);
    }
}

/// A cut as the replay counts it: its number in the whole record, its
/// group, the record of the flush whose barrier it is, the records
/// before that instant, and whether it opens as itself.
type Counted = (u64, u64, u64, u64, bool);

/// What the store's rule makes of a recorded pool with `mutants`. The
/// rule begins as a store begins on the volume the record starts from,
/// which is zeros: it reads the rings, as the record has left them so
/// far. With `restart`, a store begins again at each session's start
/// (the record's mark `session-N-begin`), as a daemon started for that
/// attachment would, and its count is carried on in the numbers.
fn replay(pool: &Pool, mutants: u64, restart: bool) -> Vec<Counted> {
    let labels = Labels::for_size(pool.dev_bytes);
    let rings: Vec<_> = labels.rings().collect();
    // The rings as the record has left them so far.
    let mut image: Vec<Vec<u8>> = rings
        .iter()
        .map(|_| vec![0; skyzfs_uberblock::RING_SIZE as usize])
        .collect();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let begin = |image: &[Vec<u8>]| {
        let mut rule = CutRule::new(pool.dev_bytes, Mutants(mutants));
        for (ring, bytes) in rings.iter().zip(image) {
            rule.read_at_start(ring.start, bytes);
        }
        rule.watch(Box::new({
            let heard = heard.clone();
            move |seen| {
                if let Seen::Cut(id, cut) = seen {
                    heard.lock().expect("lock").push((id, cut));
                }
            }
        }));
        rule
    };
    let mut rule = begin(&image);
    let mut counted: Vec<Counted> = Vec::new();
    let mut take = |heard: &std::sync::Mutex<Vec<(RequestId, Cut)>>| {
        let before = counted.len() as u64;
        for (id, cut) in heard.lock().expect("lock").drain(..) {
            let (record, after) = (id.0 / 3, id.0 % 3);
            // A flush before its write's data, or alone: the record is
            // not in the cut. The flush after a write with FUA: it is.
            let prefix = if after == 2 { record + 1 } else { record };
            counted.push((before + cut.number, cut.group, record, prefix, cut.itself));
        }
    };
    // A record's write is request 3i, a flush before it 3i+1, a flush
    // after it 3i+2; the records before an instant follow from which.
    let flush = |rule: &mut CutRule, id: u64| {
        let id = RequestId(id);
        rule.received(id, &Op::Flush);
        rule.barrier(id);
        rule.ended(id);
    };
    for record in &pool.records {
        let i = record.i;
        let flags: Vec<&str> = record.op.split('+').collect();
        match flags[0] {
            "write" => {
                if flags.contains(&"flush") {
                    flush(&mut rule, 3 * i + 1);
                }
                let blocks = usize::try_from(record.bytes).expect("a size") / BLOCK as usize;
                let data = match &record.data {
                    Some(bytes) => {
                        for (ring, kept) in rings.iter().zip(image.iter_mut()) {
                            let from = record.offset.max(ring.start);
                            let to = (record.offset + record.bytes).min(ring.end());
                            if from < to {
                                let (a, b) =
                                    ((from - ring.start) as usize, (to - ring.start) as usize);
                                let c = (from - record.offset) as usize;
                                kept[a..b].copy_from_slice(&bytes[c..c + (b - a)]);
                            }
                        }
                        Buf::from_blocks(bytes).unwrap_or_else(|| {
                            panic!("{}: record {i} is not whole blocks", pool.name)
                        })
                    }
                    None => Buf::zeroed(blocks),
                };
                let id = RequestId(3 * i);
                rule.received(
                    id,
                    &Op::Write {
                        offset: record.offset,
                        data,
                    },
                );
                rule.applied(id);
                rule.ended(id);
                if flags.contains(&"fua") {
                    flush(&mut rule, 3 * i + 2);
                }
            }
            "flush" => flush(&mut rule, 3 * i + 1),
            "discard" => {
                let id = RequestId(3 * i);
                rule.received(
                    id,
                    &Op::Discard {
                        offset: record.offset,
                        len: record.bytes,
                    },
                );
                rule.ended(id);
            }
            "mark" => {
                let session =
                    record.text.starts_with("session-") && record.text.ends_with("-begin");
                if restart && session {
                    take(&heard);
                    rule = begin(&image);
                }
            }
            other => panic!(
                "{}: record {i}: an op this test does not know: {other}",
                pool.name
            ),
        }
    }
    take(&heard);
    counted
}

/// The reference's cuts of a recorded pool, with whether each opens as
/// itself: those whose volume `zdb -e -u` opened at another group (the
/// check's `cut_N FAIL ... zdb selects group G`) do not, and every
/// other one does, since the check passed it at its own group. A cut
/// that failed the check for another reason is not one this test can
/// judge.
fn reference(pool: &Pool) -> Vec<Counted> {
    let text = std::fs::read_to_string(pool.dir.join("cuts.tsv")).expect("cuts");
    assert_eq!(
        text.lines().next(),
        Some(FORMAT),
        "{}: the format",
        pool.name
    );
    let check = std::fs::read_to_string(pool.dir.join("check.tsv")).expect("check");
    let elsewhere: Vec<u64> = rows(&check)
        .into_iter()
        .filter(|c| c[0].starts_with("cut_") && c.get(1).is_some_and(|v| v == "FAIL"))
        .map(|c| {
            assert!(
                c.get(2)
                    .is_some_and(|why| why.contains("zdb selects group")),
                "{}: a cut failed the check for another reason: {c:?}",
                pool.name
            );
            c[0]["cut_".len()..].parse().expect("a cut's number")
        })
        .collect();
    // n txg prefix ub_record flush_record rootbp.
    let wanted: Vec<Counted> = rows(&text)
        .into_iter()
        .map(|c| {
            let n = |at: usize| c[at].parse::<u64>().expect("a number");
            (n(0), n(1), n(4), n(2), !elsewhere.contains(&n(0)))
        })
        .collect();
    assert!(!wanted.is_empty(), "{}: no cut", pool.name);
    wanted
}

/// The store's rule makes the reference's cuts from each recorded pool:
/// the same count, and each cut with the same group at the barrier of
/// the same flush. Each says it opens as itself exactly where the
/// check opened the volume at the cut's own group: in the rewind, cuts
/// 23 to 28 (the new history's groups 39 to 48, whose volumes open at
/// the abandoned history's 51) do not, and the new history's own 51,
/// cut 29, does (design §5.2, "Whether a cut opens as itself").
#[test]
fn the_rule_makes_the_reference_cuts_of_every_recorded_pool() {
    for pool in pools() {
        assert_eq!(replay(&pool, 0, false), reference(&pool), "{}", pool.name);
    }
}

/// The same with a store begun again at each session's start, as a
/// daemon started for that attachment would be: it reads the rings, and
/// so sees the abandoned history's uberblocks that were written before
/// it began. A start that does not read them (the defect) is noticed:
/// it takes the rewind's cuts 23 to 28 to open as themselves.
///
/// The other defect of the order, the group alone, does not show in
/// these recordings, and the store's own test is what notices it
/// (`tests/cuts.rs`, `a_group_written_twice_opens_as_its_later_state`).
/// Two states of one group meet in view only after a rewind, and the
/// slot an uberblock goes to is its group modulo the ring's slots: the
/// new history's 51 is written over the abandoned 51 in every label
/// before the flush that cuts it. A multihost pool keeps the ring's last
/// slot for its own writes, where an abandoned state can outlive that;
/// the `multihost` recording, not in yet, is the one that could show it.
#[test]
fn a_store_begun_at_each_session_says_which_cuts_open_as_themselves() {
    let pools = pools();
    for pool in &pools {
        assert_eq!(replay(pool, 0, true), reference(pool), "{}", pool.name);
    }
    let unread: Vec<Vec<Counted>> = pools
        .iter()
        .map(|pool| replay(pool, Mutants::RINGS_UNREAD, true))
        .collect();
    let right: Vec<Vec<Counted>> = pools.iter().map(reference).collect();
    assert_ne!(
        unread, right,
        "a start that does not read the rings is not noticed"
    );
}
