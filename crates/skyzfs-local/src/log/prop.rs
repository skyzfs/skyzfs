//! The write log against a model, over random programs.
//!
//! A program appends, syncs, reclaims, kills the process, cuts the
//! power and injects write and sync errors, in any order. The model is
//! the list of records the log has acknowledged and how many of them a
//! successful sync has covered. After every recovery the log must hold:
//!
//! - **after a kill**, every acknowledged record, in order, and nothing
//!   else, except possibly the one record whose append failed;
//! - **after a power cut**, or once a sync has failed, a prefix of
//!   that, which contains everything a successful sync covered, with
//!   every record whole;
//! - **after a second power cut straight after a recovery**, the same
//!   records again;
//! - **later**, never anything that an earlier recovery dropped: the
//!   model forgets what a recovery drops, and every later comparison is
//!   exact.

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, RngAlgorithm, TestRng, TestRunner};

use super::*;
use crate::sim::{LoseAll, SeededCut, SimDisk};

const SEG: u64 = 32 << 10;

#[derive(Debug, Clone)]
enum Op {
    /// Append a record: a kind, a payload length, a byte to fill it with.
    Append(u8, usize, u8),
    Sync,
    /// Reclaim up to the record this number selects.
    Reclaim(usize),
    /// Hold at the record this number selects, or lift the hold.
    Hold(Option<usize>),
    /// The process dies; reopen.
    Kill,
    /// Power cut from this seed, with or without garbage; reopen.
    Cut(u64, bool),
    /// The write after so many fails.
    FailWrite(u64),
    /// The sync after so many fails.
    FailSync(u64),
    /// The operating system drops its clean pages.
    DropCaches,
}

fn op() -> impl Strategy<Value = Op> {
    let len = prop_oneof![
        Just(1usize),
        Just(100),
        Just(448),
        Just(1000),
        Just(4032),
        Just(4096),
        Just(8000),
    ];
    prop_oneof![
        12 => (0..3u8, len, 1..=255u8).prop_map(|(k, l, f)| Op::Append(k, l, f)),
        4 => Just(Op::Sync),
        2 => any::<usize>().prop_map(Op::Reclaim),
        1 => proptest::option::of(any::<usize>()).prop_map(Op::Hold),
        2 => Just(Op::Kill),
        3 => (any::<u64>(), any::<bool>()).prop_map(|(s, g)| Op::Cut(s, g)),
        1 => (0..4u64).prop_map(Op::FailWrite),
        1 => (0..4u64).prop_map(Op::FailSync),
        1 => Just(Op::DropCaches),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rec {
    start: Lsn,
    end: Lsn,
    kind: Kind,
    offset: u64,
    length: u64,
    data: Vec<u8>,
}

fn read_all(log: &Log<SimDisk>) -> Result<Vec<Rec>, LogError> {
    let mut out = Vec::new();
    log.replay(log.floor(), log.tail(), &mut |r| {
        out.push(Rec {
            start: r.start,
            end: r.end,
            kind: r.kind,
            offset: r.offset,
            length: r.length,
            data: r.data.to_vec(),
        });
    })?;
    Ok(out)
}

#[derive(Debug, Default)]
struct Model {
    /// The acknowledged records from the log's start on.
    records: Vec<Rec>,
    /// How many of them a successful sync has covered.
    synced: usize,
    /// The log has failed.
    failed: bool,
    /// A sync has failed since the last recovery: unsynced records may
    /// be gone without a power cut.
    uncertain: bool,
    /// The content of the append that failed: it may have been written.
    extra: Option<(Kind, u64, u64, Vec<u8>)>,
    /// Where the hold is.
    hold: Option<Lsn>,
    /// A reclaim failed while writing the new start: the log may start
    /// there. This many leading records may be gone.
    maybe_reclaimed: usize,
}

struct Run {
    disk: SimDisk,
    log: Option<Log<SimDisk>>,
    model: Model,
    mutants: Mutants,
}

impl Run {
    fn log(&mut self) -> Result<&mut Log<SimDisk>, String> {
        self.log.as_mut().ok_or_else(|| "no log".to_owned())
    }

    /// Open the log again after the process or the power was lost, and
    /// compare what it holds with the model. `strict`: nothing may be
    /// missing.
    fn recover(&mut self, strict: bool) -> Result<(), String> {
        self.log = None;
        let mut strict = strict && !self.model.uncertain;
        let mut attempts = 0;
        let (log, recovery) = loop {
            self.disk.kill();
            let options = Options {
                acknowledge_failure: true,
                mutants: self.mutants,
            };
            match Log::open(self.disk.clone(), options) {
                Ok(opened) => break opened,
                // An injected error that had not struck yet struck the
                // recovery. What it was syncing may be gone.
                Err(LogError::Io { .. }) if attempts < 3 => {
                    attempts += 1;
                    strict = false;
                    self.disk.clear_faults();
                }
                Err(other) => return Err(format!("open failed: {other}")),
            }
        };
        if recovery.marker_found && !self.model.failed && attempts == 0 {
            return Err("a failure marker on a log that had not failed".into());
        }
        let held = read_all(&log).map_err(|e| format!("replay after recovery: {e}"))?;
        let model = &mut self.model;
        let mut all = model.records.clone();
        // A reclaim that failed may have moved the start all the same.
        // It only gives up what was durable, so nothing else changes.
        let reclaimed = model.maybe_reclaimed;
        if reclaimed > 0 && held.first() != all.first() {
            all.drain(..reclaimed);
            model.records.drain(..reclaimed);
            model.synced -= reclaimed;
        }
        let with_extra = held.len() == all.len() + 1;
        if with_extra {
            let Some((kind, offset, length, data)) = model.extra.clone() else {
                return Err(format!(
                    "the log holds a record nobody appended: {:?}",
                    held.last().map(|r| (r.start, r.kind, r.offset))
                ));
            };
            let last = &held[held.len() - 1];
            all.push(Rec {
                start: last.start,
                end: last.end,
                kind,
                offset,
                length,
                data,
            });
        }
        if !all.starts_with(&held) {
            let at = held
                .iter()
                .zip(&all)
                .position(|(a, b)| a != b)
                .unwrap_or(all.len());
            return Err(format!(
                "the log is not a prefix of what was appended: it holds {} records, {} were \
                 appended, and they differ at record {at}",
                held.len(),
                all.len()
            ));
        }
        if held.len() < model.synced {
            return Err(format!(
                "a sync covered {} records and the log holds {}",
                model.synced,
                held.len()
            ));
        }
        if strict && held.len() < model.records.len() {
            return Err(format!(
                "after a kill the log holds {} of {} appended records",
                held.len(),
                model.records.len()
            ));
        }
        if recovery.end > log.tail() || log.durable() != log.tail() {
            return Err("after recovery the log's tail is not durable".into());
        }
        *model = Model {
            synced: held.len(),
            records: held,
            ..Model::default()
        };
        self.log = Some(log);
        Ok(())
    }

    /// The log, read back, is exactly the model.
    fn compare(&mut self) -> Result<(), String> {
        if self.model.uncertain {
            return Ok(());
        }
        let held = read_all(self.log()?).map_err(|e| format!("replay: {e}"))?;
        if held != self.model.records {
            return Err(format!(
                "the log holds {} records, the model {}",
                held.len(),
                self.model.records.len()
            ));
        }
        Ok(())
    }

    fn step(&mut self, op: &Op) -> Result<(), String> {
        match *op {
            Op::Append(kind, len, fill) => {
                let kind = [Kind::Write, Kind::WriteZeroes, Kind::Discard][usize::from(kind)];
                let data = if kind == Kind::Write {
                    vec![fill; len]
                } else {
                    Vec::new()
                };
                let (offset, length) = (u64::from(fill) * 4096, len as u64);
                let failed = self.model.failed;
                match self.log()?.append(kind, offset, length, &data) {
                    Ok(at) if !failed => self.model.records.push(Rec {
                        start: at.start,
                        end: at.end,
                        kind,
                        offset,
                        length,
                        data,
                    }),
                    Ok(_) => return Err("an append succeeded on a failed log".into()),
                    Err(LogError::Failed) if failed => {}
                    Err(LogError::Io { .. }) if !failed => {
                        self.model.failed = true;
                        self.model.extra = Some((kind, offset, length, data));
                    }
                    Err(other) => return Err(format!("append: {other}")),
                }
            }
            Op::Sync => {
                let failed = self.model.failed;
                let tail = self.log()?.tail();
                match self.log()?.sync() {
                    Ok(at) if !failed && at == tail => {
                        self.model.synced = self.model.records.len();
                    }
                    Ok(at) => return Err(format!("sync returned {at} (failed log: {failed})")),
                    Err(LogError::Failed) if failed => {}
                    Err(LogError::Io { .. }) if !failed => {
                        self.model.failed = true;
                        self.model.uncertain = true;
                    }
                    Err(other) => return Err(format!("sync: {other}")),
                }
            }
            Op::Hold(pick) => {
                let hold = pick.map(|pick| self.boundary(pick)).transpose()?;
                self.log()?.set_hold(hold);
                self.model.hold = hold;
            }
            Op::Reclaim(pick) => {
                let before = self.boundary(pick)?;
                let failed = self.model.failed;
                let log = self.log()?;
                let (floor, durable) = (log.floor(), log.durable());
                let limit = self.model.hold.map_or(before, |hold| hold.min(before));
                let want = limit.min(durable).max(floor);
                match self.log()?.reclaim(before) {
                    Ok(start) if !failed && start == want => {
                        let dropped = self
                            .model
                            .records
                            .iter()
                            .take_while(|r| r.start < start)
                            .count();
                        if dropped > self.model.synced {
                            return Err("reclaim passed the durable position".into());
                        }
                        self.model.records.drain(..dropped);
                        self.model.synced -= dropped;
                    }
                    Ok(start) => return Err(format!("reclaim returned {start}, expected {want}")),
                    Err(LogError::Failed) if failed => {}
                    Err(LogError::Io { .. }) if !failed => {
                        self.model.failed = true;
                        self.model.uncertain = true;
                        self.model.maybe_reclaimed = self
                            .model
                            .records
                            .iter()
                            .take_while(|r| r.start < want)
                            .count();
                    }
                    Err(other) => return Err(format!("reclaim: {other}")),
                }
            }
            Op::Kill => self.recover(true)?,
            Op::Cut(seed, garbage) => {
                self.log = None;
                if garbage {
                    self.disk.power_cut(SeededCut::with_garbage(seed));
                } else {
                    self.disk.power_cut(SeededCut::new(seed));
                }
                self.recover(false)?;
                // What a recovery returned survives a power cut straight
                // after it.
                if seed % 2 == 0 {
                    self.log = None;
                    self.disk.power_cut(LoseAll);
                    self.model.synced = self.model.records.len();
                    self.recover(true)?;
                }
            }
            Op::FailWrite(after) => self.disk.fail_write(after, false),
            Op::FailSync(after) => self.disk.fail_sync(after, false),
            Op::DropCaches => self.disk.drop_caches(),
        }
        self.compare()
    }

    /// A record boundary chosen by `pick`: the start of a record, or the
    /// end of the log.
    fn boundary(&mut self, pick: usize) -> Result<Lsn, String> {
        let tail = self.log()?.tail();
        let records = &self.model.records;
        Ok(records
            .get(pick % (records.len() + 1))
            .map_or(tail, |r| r.start))
    }
}

/// Run one program against a log with `mutants`.
fn run(align: u32, mutants: Mutants, seed: u64, program: &[Op]) -> Result<(), String> {
    let disk = SimDisk::new(seed);
    let identity = Identity {
        volume: *b"property-testing",
        device_size: 1 << 30,
        block_size: 4096,
        align,
        segment_size: SEG,
    };
    drop(Log::create(disk.clone(), identity).map_err(|e| e.to_string())?);
    let mut run = Run {
        disk,
        log: None,
        model: Model::default(),
        mutants,
    };
    run.recover(true)?;
    for (i, op) in program.iter().enumerate() {
        run.step(op)
            .map_err(|why| format!("step {i} ({op:?}): {why}"))?;
    }
    // Whatever happened: a kill at the end loses nothing that the model
    // still expects.
    run.recover(true)
}

fn program() -> impl Strategy<Value = (u64, Vec<Op>)> {
    (any::<u64>(), proptest::collection::vec(op(), 1..60))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// The model's four properties (see the module documentation), with
    /// records aligned to 64 bytes.
    #[test]
    fn the_log_matches_the_model((seed, program) in program()) {
        if let Err(why) = run(64, Mutants::default(), seed, &program) {
            return Err(TestCaseError::fail(why));
        }
    }

    /// The same, with records aligned to 4096 bytes.
    #[test]
    fn the_log_matches_the_model_at_4096((seed, program) in program()) {
        if let Err(why) = run(4096, Mutants::default(), seed, &program) {
            return Err(TestCaseError::fail(why));
        }
    }
}

/// Whether random programs notice a log with `mutants`, within `cases`.
fn noticed(mutants: u32, cases: u32) -> bool {
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig {
            cases,
            max_shrink_iters: 0,
            failure_persistence: None,
            ..ProptestConfig::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    runner
        .run(&program(), |(seed, program)| {
            for align in [64, 4096] {
                if let Err(why) = run(align, Mutants(mutants), seed, &program) {
                    return Err(TestCaseError::fail(why));
                }
            }
            Ok(())
        })
        .is_err()
}

/// The random programs reach the defects, not only the scenarios that
/// were written for them: each of these broken logs is noticed within a
/// fixed number of programs. (The scenario tests say which defect needs
/// which history; the defects that only show together with another one,
/// or only after two particular power cuts, are theirs.)
#[test]
fn random_programs_notice_broken_logs() {
    let broken = [
        ("a torn record accepted", Mutants::NO_DATA_CHECK),
        ("a record accepted after a gap", Mutants::ACCEPT_AFTER_GAP),
        ("a sync that returns early", Mutants::SYNC_EARLY),
        (
            "a sync of the current segment only",
            Mutants::SYNC_ONLY_TAIL,
        ),
        ("a sync without the directory", Mutants::SYNC_NO_DIR),
        ("a failed sync retried", Mutants::RETRY_SYNC),
        ("recovery through the cache", Mutants::VALIDATE_CACHED),
        ("recovery without a sync", Mutants::NO_SYNC_BEFORE_VALIDATE),
        (
            "reuse before the start is durable",
            Mutants::REUSE_BEFORE_FLOOR_SYNC,
        ),
        ("a reclaim that ignores the hold", Mutants::IGNORE_HOLD),
    ];
    let missed: Vec<&str> = broken
        .iter()
        .filter(|(_, mutants)| !noticed(*mutants, 1500))
        .map(|(name, _)| *name)
        .collect();
    assert!(missed.is_empty(), "not noticed: {missed:?}");
}
