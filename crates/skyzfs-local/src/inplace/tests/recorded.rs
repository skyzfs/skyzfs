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
//! with the reference in full.
//!
//! What the recordings do not show: none of the rule's defects changes
//! what these five pools make (`life`, `rewind`, `config`, `stray`,
//! `autotrim`; probed on 2026-11-14). No flush in them falls between
//! one group's uberblock writes, so a repeat never follows a cut and
//! counting repeats makes no other cut; no uberblock of group 0 is in a
//! record; and the rewind's group used twice (51) never meets its
//! earlier self as the mark or the newest cut, so a repeat told by the
//! group alone makes the same cuts. The store's own tests are what
//! notice those defects (`tests/cuts.rs`). The `multihost` recording,
//! not in yet, is the one whose stale writes are repeats after a cut.
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
            let data = (data != "-" && op.starts_with("write")).then(|| unhex(data));
            Record {
                i: number(i),
                op: op.clone(),
                offset: number(offset),
                bytes: number(bytes),
                data,
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

/// What the store's rule makes of a recorded pool, in the columns of
/// `cuts.tsv` it can say: the cut's number, its group, the record of
/// the flush whose barrier it is, and the records before that instant.
fn counted(pool: &Pool) -> Vec<[u64; 4]> {
    let cuts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut rule = CutRule::new(pool.dev_bytes, Mutants(0));
    rule.watch(Box::new({
        let cuts = cuts.clone();
        move |seen| {
            if let Seen::Cut(id, cut) = seen {
                cuts.lock().expect("lock").push((id, cut));
            }
        }
    }));
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
                    Some(bytes) => Buf::from_blocks(bytes)
                        .unwrap_or_else(|| panic!("{}: record {i} is not whole blocks", pool.name)),
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
            "mark" => {}
            other => panic!(
                "{}: record {i}: an op this test does not know: {other}",
                pool.name
            ),
        }
    }
    let cuts = cuts.lock().expect("lock").clone();
    cuts.into_iter()
        .map(|(id, cut)| {
            let (record, after) = (id.0 / 3, id.0 % 3);
            // A flush before its write's data, or alone: the record is
            // not in the cut. The flush after a write with FUA: it is.
            let prefix = if after == 2 { record + 1 } else { record };
            [cut.number, cut.group, record, prefix]
        })
        .collect()
}

/// The store's rule makes the reference's cuts from each recorded pool:
/// the same count, and each cut with the same group at the barrier of
/// the same flush. The rewind's cuts 23 to 28 are the reference's cuts
/// by §5.2 although their volumes open at the abandoned history's group
/// (its `check.tsv`); this test holds the rule to the reference as it
/// is, and what a cut after a rewind is remains the lead's decision.
#[test]
fn the_rule_makes_the_reference_cuts_of_every_recorded_pool() {
    for pool in pools() {
        let text = std::fs::read_to_string(pool.dir.join("cuts.tsv")).expect("cuts");
        assert_eq!(
            text.lines().next(),
            Some(FORMAT),
            "{}: the format",
            pool.name
        );
        // n txg prefix ub_record flush_record rootbp.
        let wanted: Vec<[u64; 4]> = rows(&text)
            .into_iter()
            .map(|c| {
                let n = |at: usize| c[at].parse::<u64>().expect("a number");
                [n(0), n(1), n(4), n(2)]
            })
            .collect();
        assert!(!wanted.is_empty(), "{}: no cut", pool.name);
        assert_eq!(counted(&pool), wanted, "{}", pool.name);
    }
}
