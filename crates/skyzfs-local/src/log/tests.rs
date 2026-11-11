//! The write log's tests, on the simulated disk.
//!
//! Most are *scenarios*: a function that drives a log through a history
//! and says whether the log kept its promise. A scenario is run in four
//! environments (record alignment 64 or 4096; a sector lost at a power
//! cut reads as its old content or as garbage) and must hold in all of
//! them. Then it is run against a log with a deliberate defect (a
//! mutant), and must fail: a test that cannot fail proves nothing. Where
//! a defect can only show in some environments (a resurrection needs the
//! old content to come back), the test says in which.
//!
//! Some of the log's defences overlap on purpose. A defect in one of
//! them is then invisible while the other stands, and its test removes
//! both and shows that each alone is enough.

use super::*;
use crate::format::Key;
use crate::io::SECTOR;
use crate::sim::{BySector, Cut, Fate, LoseAll, SeededCut, SimDisk};

const SEG: u64 = 64 << 10;

/// The write unit of the logs these scenarios use.
const BLOCK: u64 = 4096;

#[derive(Debug, Clone, Copy)]
struct Env {
    mutants: Mutants,
    align: u32,
    /// A sector lost at a power cut reads as garbage, not as its old
    /// content.
    garbage: bool,
    seed: u64,
}

type Outcome = Result<(), String>;
type SimLog = Log<SimDisk>;

fn s<T>(result: Result<T, LogError>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

impl Env {
    fn identity(&self) -> Identity {
        Identity {
            volume: *b"volume-under-tst",
            device_size: 1 << 30,
            block_size: 4096,
            align: self.align,
            segment_size: SEG,
            write_unit: BLOCK as u32,
        }
    }

    /// A payload that makes a record a whole number of sectors: 8 at
    /// alignment 64, 16 at 4096.
    fn payload(&self) -> usize {
        4096 - (self.align as usize % 4096)
    }

    fn options(&self) -> Options {
        Options {
            acknowledge_failure: false,
            mutants: self.mutants,
        }
    }

    /// The key of the logs these tests make. With the defect of a key
    /// that everybody knows, that key.
    fn key(&self) -> Key {
        Key(*b"a key for tests!")
    }

    /// A new log on a new disk, opened with the environment's defects.
    fn fresh(&self) -> Result<(SimDisk, SimLog), String> {
        let disk = SimDisk::new(self.seed);
        let log = s(Log::create_keyed(
            disk.clone(),
            self.identity(),
            self.key(),
            self.options(),
        ))?;
        Ok((disk, log))
    }

    /// The process is lost and the log opened again.
    fn reopen(&self, disk: &SimDisk) -> Result<(SimLog, Recovery), String> {
        disk.kill();
        s(Log::open(disk.clone(), self.options()))
    }

    /// The same, acknowledging a failure marker if there is one.
    fn reopen_ack(&self, disk: &SimDisk) -> Result<(SimLog, Recovery), String> {
        disk.kill();
        s(Log::open(
            disk.clone(),
            Options {
                acknowledge_failure: true,
                ..self.options()
            },
        ))
    }

    fn lost(&self) -> Fate {
        if self.garbage {
            Fate::Garbage
        } else {
            Fate::Durable
        }
    }

    /// A power cut that loses exactly the sectors in `lose` and keeps
    /// every other unsynced sector, every name and every length.
    fn cut(&self, disk: &SimDisk, lose: &[(String, u64)]) {
        let lost = self.lost();
        disk.power_cut(BySector(|name: Option<&str>, index| {
            let hit = lose
                .iter()
                .any(|(n, i)| Some(n.as_str()) == name && *i == index);
            if hit {
                lost
            } else {
                Fate::Latest
            }
        }));
    }
}

/// Append a write whose payload is `tag` repeated.
fn put(log: &mut SimLog, env: &Env, tag: u8) -> Result<Appended, String> {
    s(log.append(
        Kind::Write,
        u64::from(tag) * 4096,
        env.payload() as u64,
        &vec![tag; env.payload()],
    ))
}

/// Append the smallest record there is: a discard, tagged by its offset.
fn put_small(log: &mut SimLog, tag: u8) -> Result<Appended, String> {
    s(log.append(Kind::Discard, u64::from(tag) * 4096, 4096, &[]))
}

/// The tags of the records between two positions. A record that is not
/// one `put` or `put_small` wrote, whole, shows as 0xFF.
fn tags_between(log: &SimLog, from: Lsn, to: Lsn) -> Result<Vec<u8>, LogError> {
    let mut tags = Vec::new();
    log.replay_unsynced(from, to, &mut |record| {
        let tag = match record.kind {
            Kind::Discard => (record.offset / 4096) as u8,
            _ => record.data.first().copied().unwrap_or(0xFF),
        };
        let whole = match record.kind {
            Kind::Discard => record.length == 4096 && record.data.is_empty(),
            _ => {
                record.kind == Kind::Write
                    && record.offset == u64::from(tag) * 4096
                    && record.length == record.data.len() as u64
                    && record.data.iter().all(|b| *b == tag)
            }
        };
        tags.push(if whole { tag } else { 0xFF });
    })?;
    Ok(tags)
}

fn tags(log: &SimLog) -> Result<Vec<u8>, String> {
    s(tags_between(log, log.floor(), log.tail()))
}

fn expect(log: &SimLog, want: &[u8], what: &str) -> Outcome {
    let got = tags(log)?;
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: the log holds {got:?}, expected {want:?}"))
    }
}

/// The file that holds a position, while no segment file has been
/// reused: the files are created in the order of the log.
fn file_of(at: Lsn) -> String {
    slot_name((at.0 / SEG) as usize)
}

/// The sector that holds a record's header.
fn header_sector(record: Appended) -> (String, u64) {
    (file_of(record.start), record.start.0 % SEG / SECTOR)
}

/// The sectors of a record after the one that holds its header.
fn body_sectors(record: Appended) -> Vec<(String, u64)> {
    let first = record.start.0 % SEG / SECTOR + 1;
    let last = (record.end.0 - 1) % SEG / SECTOR;
    (first..=last).map(|i| (file_of(record.start), i)).collect()
}

fn all_sectors(record: Appended) -> Vec<(String, u64)> {
    let mut sectors = vec![header_sector(record)];
    sectors.extend(body_sectors(record));
    sectors
}

/// Append records, with tags counting up from `*tag`, until one starts a
/// new segment. Returns them; the last is the first of the new segment.
fn fill_to_roll(log: &mut SimLog, env: &Env, tag: &mut u8) -> Result<Vec<(u8, Appended)>, String> {
    let base = log.tail().0 / SEG;
    let mut out = Vec::new();
    loop {
        let record = put(log, env, *tag)?;
        out.push((*tag, record));
        *tag += 1;
        if record.start.0 / SEG != base {
            return Ok(out);
        }
    }
}

fn environments(mutants: u64) -> Vec<Env> {
    let mut out = Vec::new();
    for align in [64, 4096] {
        for garbage in [false, true] {
            out.push(Env {
                mutants: Mutants(mutants),
                align,
                garbage,
                seed: 0,
            });
        }
    }
    out
}

/// The scenario holds for the log as it is, in every environment.
fn holds(scenario: fn(&Env) -> Outcome) {
    for env in environments(0) {
        if let Err(why) = scenario(&env) {
            panic!("the log broke its promise in {env:?}: {why}");
        }
    }
}

/// The scenario holds for the log as it is, and fails for the log with
/// `mutants` in every environment that `where_` selects.
fn caught(scenario: fn(&Env) -> Outcome, mutants: u64, where_: fn(&Env) -> bool) {
    holds(scenario);
    let mut ran = 0;
    for env in environments(mutants).into_iter().filter(where_) {
        ran += 1;
        if scenario(&env).is_ok() {
            panic!("the defect {mutants:#x} was not noticed in {env:?}");
        }
    }
    assert!(ran > 0);
}

/// The scenario still holds with `mutants`: another defence covers.
fn covered(scenario: fn(&Env) -> Outcome, mutants: u64) {
    for env in environments(mutants) {
        if let Err(why) = scenario(&env) {
            panic!("the defect {mutants:#x} alone already breaks {env:?}: {why}");
        }
    }
}

fn everywhere(_: &Env) -> bool {
    true
}

/// Only where a lost sector reads as its old content: what a
/// resurrection needs.
fn old_content(env: &Env) -> bool {
    !env.garbage
}

fn garbage(env: &Env) -> bool {
    env.garbage
}

// ---------------------------------------------------------------------
// The API, without failures.

/// Append and replay: every kind comes back with its fields and payload,
/// positions are contiguous, a sub-range replays exactly its records,
/// and requests the log cannot serve are refused.
#[test]
fn records_round_trip() {
    fn scenario(env: &Env) -> Outcome {
        let (_disk, mut log) = env.fresh()?;
        if log.tail() != log.durable() || !tags(&log)?.is_empty() {
            return Err("a new log is not empty".into());
        }
        let a = s(log.append(Kind::Write, 8192, 3, b"abc"))?;
        let start = a.start;
        if start < log.floor() || start.0 % BLOCK != 0 {
            return Err("the first record is not on a unit boundary".into());
        }
        let b = s(log.append(Kind::WriteZeroes, 4096, 1 << 20, &[]))?;
        let c = s(log.append(Kind::Discard, 0, 8192, &[]))?;
        let d = put(&mut log, env, 7)?;
        if a.end != b.start || b.end != c.start || c.end != d.start {
            return Err("positions are not contiguous".into());
        }
        if d.end != log.tail() {
            return Err("the tail is not the end of the last record".into());
        }
        let mut seen = Vec::new();
        s(log.replay_unsynced(start, d.end, &mut |r| {
            seen.push((r.start, r.end, r.kind, r.offset, r.length, r.data.to_vec()));
        }))?;
        let want = vec![
            (a.start, a.end, Kind::Write, 8192, 3, b"abc".to_vec()),
            (b.start, b.end, Kind::WriteZeroes, 4096, 1 << 20, vec![]),
            (c.start, c.end, Kind::Discard, 0, 8192, vec![]),
            (
                d.start,
                d.end,
                Kind::Write,
                7 * 4096,
                env.payload() as u64,
                vec![7; env.payload()],
            ),
        ];
        if seen != want {
            return Err(format!("replayed {seen:?}"));
        }
        let mut kinds = Vec::new();
        s(log.replay_unsynced(b.start, c.end, &mut |r| kinds.push(r.kind)))?;
        if kinds != [Kind::WriteZeroes, Kind::Discard] {
            return Err(format!("a sub-range replayed {kinds:?}"));
        }
        s(log.replay_unsynced(d.end, d.end, &mut |_| kinds.push(Kind::Pad)))?;
        if kinds.len() != 2 {
            return Err("an empty range replayed something".into());
        }
        let refused = [
            log.replay_unsynced(Lsn(start.0 + 8), d.end, &mut |_| {}),
            log.replay_unsynced(start, Lsn(d.end.0 - 8), &mut |_| {}),
            log.replay_unsynced(start, Lsn(d.end.0 + 64), &mut |_| {}),
            log.replay_unsynced(c.start, b.start, &mut |_| {}),
        ];
        if !refused
            .iter()
            .all(|r| matches!(r, Err(LogError::BadPosition(_))))
        {
            return Err(format!("bad positions gave {refused:?}"));
        }
        let max = log.identity().max_data() as usize;
        let refused = [
            log.append(Kind::Write, 0, 0, &[]),
            log.append(Kind::Write, 0, 0, &vec![0; max + 1]),
            log.append(Kind::Discard, 0, 1, b"x"),
            log.append(Kind::Pad, 0, 0, &[]),
        ];
        if !refused
            .iter()
            .all(|r| matches!(r, Err(LogError::BadRecord { .. })))
        {
            return Err(format!("bad records gave {refused:?}"));
        }
        // The largest record fits.
        s(log.append(Kind::Write, 0, max as u64, &vec![5; max]))?;
        Ok(())
    }
    holds(scenario);
}

/// Create and open: a log is created once, opened only where there is
/// one, and only with parameters that can describe one.
#[test]
fn create_and_open_check_what_is_there() {
    let env = environments(0)[0];
    let disk = SimDisk::new(0);
    assert_eq!(
        Log::open(disk.clone(), Options::default()).unwrap_err(),
        LogError::NotFound
    );
    let bad = Identity {
        align: 512,
        ..env.identity()
    };
    assert_eq!(
        Log::create(disk.clone(), bad).unwrap_err(),
        LogError::InvalidConfig
    );
    let log = Log::create(disk.clone(), env.identity()).unwrap();
    assert_eq!(*log.identity(), env.identity());
    assert_eq!(log.segment_files(), 1);
    assert!(!log.is_failed());
    assert_eq!(
        Log::create(disk.clone(), env.identity()).unwrap_err(),
        LogError::Exists
    );
}

/// Replay between boundaries. A record's start and its end are both
/// boundaries, and after a sync or a recovery the start of a record is
/// not the end of the one before it: the record begins at the next unit
/// boundary. A replay up to a record's start gives everything before
/// that record, a replay from it gives the record and what follows, and
/// the range between the end of one record and the start of the next is
/// empty: behind a sync, across a segment switch, and across the seal a
/// recovery writes. A position in a gap is refused, as one inside a
/// record is. Mutant: a replay that does not stop at a record starting
/// where the range ends.
#[test]
fn replay_takes_record_starts_and_ends_also_behind_a_gap() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let floor = log.floor();
        let check = |log: &SimLog, from: Lsn, to: Lsn, want: &[u8], what: &str| match tags_between(
            log, from, to,
        ) {
            Ok(got) if got == want => Ok(()),
            other => Err(format!("{what}: from {from} to {to} gave {other:?}")),
        };
        // Behind a sync.
        let a = put_small(&mut log, 1)?;
        s(log.sync())?;
        let b = put_small(&mut log, 2)?;
        if env.align == 64 && a.end == b.start {
            return Err("no gap behind the sync".into());
        }
        check(&log, floor, b.start, &[1], "up to a start behind a sync")?;
        check(&log, b.start, b.end, &[2], "from a start behind a sync")?;
        check(&log, a.end, b.start, &[], "across the gap")?;
        check(&log, b.start, b.start, &[], "empty at a start")?;
        if a.end != b.start {
            let gap = Lsn(a.end.0 + u64::from(env.align));
            for (from, to) in [(floor, gap), (gap, b.end), (gap, gap)] {
                match log.replay_unsynced(from, to, &mut |_| {}) {
                    Err(LogError::BadPosition(at)) if at == gap => {}
                    other => return Err(format!("a position in a gap gave {other:?}")),
                }
            }
        }
        // Across a segment switch.
        let mut tag = 3;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        let (last_tag, last) = filled[filled.len() - 1];
        let (_, before) = filled[filled.len() - 2];
        let mut all = vec![1, 2];
        all.extend(filled.iter().map(|(t, _)| *t));
        let upto = &all[..all.len() - 1];
        check(
            &log,
            floor,
            last.start,
            upto,
            "up to a start in a new segment",
        )?;
        check(&log, last.start, last.end, &[last_tag], "from that start")?;
        check(&log, before.end, last.start, &[], "across the switch")?;
        check(
            &log,
            b.start,
            before.end,
            &all[1..all.len() - 1],
            "to an end",
        )?;
        // Across a recovery's seal.
        drop(log);
        let (mut log, recovery) = env.reopen(&disk)?;
        let d = put_small(&mut log, 200)?;
        if d.start <= recovery.end {
            return Err("no seal between the prefix and the next record".into());
        }
        check(&log, floor, d.start, &all, "up to a start behind a seal")?;
        check(&log, d.start, d.end, &[200], "from a start behind a seal")?;
        check(&log, recovery.end, d.start, &[], "across the seal")?;
        check(&log, last.start, d.end, &[last_tag, 200], "over the seal")
    }
    // At alignment 4096 every record starts where the one before it
    // ends, and the defect has nothing to act on.
    fn with_gaps(env: &Env) -> bool {
        env.align == 64
    }
    caught(scenario, Mutants::REPLAY_PAST_START, with_gaps);
}

/// A replay hands out only what is durable. A record that is appended
/// and not synced survives the loss of the process but not a power cut,
/// and whoever applied it to a place that outlives the log would keep
/// what the log can lose. So a replay beyond the durable position is
/// refused and visits nothing; up to it, and after a sync or an open
/// up to the tail, it works. Mutant: a replay up to the tail.
#[test]
fn a_replay_ends_at_the_durable_position() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let a = put(&mut log, env, 1)?;
        s(log.sync())?;
        let b = put(&mut log, env, 2)?;
        if log.durable() != a.end {
            return Err("the durable position is not the synced record's end".into());
        }
        let mut seen = Vec::new();
        match log.replay(log.floor(), log.tail(), &mut |r| seen.push(r.start)) {
            Err(LogError::BadPosition(at)) if at == b.end && seen.is_empty() => {}
            other => return Err(format!("beyond the durable position: {other:?}, {seen:?}")),
        }
        s(log.replay(log.floor(), log.durable(), &mut |r| seen.push(r.start)))?;
        if seen != [a.start] {
            return Err(format!("up to the durable position: {seen:?}"));
        }
        s(log.sync())?;
        seen.clear();
        s(log.replay(log.floor(), log.tail(), &mut |r| seen.push(r.start)))?;
        put(&mut log, env, 3)?;
        drop(log);
        // After an open the whole log is durable.
        let (log, _) = env.reopen(&disk)?;
        let mut count = 0;
        s(log.replay(log.floor(), log.tail(), &mut |_| count += 1))?;
        if seen != [a.start, b.start] || count != 3 {
            return Err(format!(
                "after a sync {seen:?}, after an open {count} records"
            ));
        }
        Ok(())
    }
    caught(scenario, Mutants::REPLAY_UNSYNCED, everywhere);
}

/// The start of the log is a record boundary. It is durable state: a
/// start inside a record, or in the gap in front of one, would outlive
/// the process and leave a log that cannot be read from its start. So a
/// reclaim to such a position is refused, the start stays where it was,
/// and the log reads the same after a reopen; a reclaim to the record's
/// start or end works. Mutant: a reclaim that takes any position.
#[test]
fn a_reclaim_takes_only_a_record_boundary() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let a = put(&mut log, env, 1)?;
        s(log.sync())?;
        let b = put(&mut log, env, 2)?;
        let c = put(&mut log, env, 3)?;
        s(log.sync())?;
        let floor = log.floor();
        let mut bad = vec![Lsn(b.start.0 + 8), Lsn(b.start.0 + u64::from(env.align))];
        if a.end != b.start {
            // In the gap behind the sync.
            bad.push(Lsn(a.end.0 + u64::from(env.align)));
        }
        for at in bad {
            match log.reclaim(at) {
                Err(LogError::BadPosition(refused)) if refused == at => {}
                other => return Err(format!("a reclaim to {at} gave {other:?}")),
            }
            if log.floor() != floor || log.is_failed() {
                return Err("a refused reclaim changed the log".into());
            }
        }
        expect(&log, &[1, 2, 3], "after the refused reclaims")?;
        if s(log.reclaim(b.start))? != b.start {
            return Err("a reclaim to a record's start did not take it".into());
        }
        if s(log.reclaim(b.end))? != b.end || b.end != c.start {
            return Err("a reclaim to a record's end did not take it".into());
        }
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[3], "after a reopen")
    }
    caught(scenario, Mutants::RECLAIM_UNCHECKED, everywhere);
}

/// A record's payload can be read back as soon as it is appended,
/// before any sync: whole or in part, by the position the append
/// returned. A position that is not the start of a write record, a
/// range beyond the payload, a record below the start of the log and a
/// failed log are refused.
#[test]
fn a_payload_is_read_back_before_it_is_synced() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let first = put(&mut log, env, 1)?;
        let small = put_small(&mut log, 2)?;
        let a = s(log.append(Kind::Write, 8192, data.len() as u64, &data))?;
        if log.durable() >= a.end {
            return Err("the record is durable already".into());
        }
        for (skip, len) in [(0, data.len()), (0, 1), (4096, 4096), (9_999, 1)] {
            let mut buf = vec![0xEE; len];
            s(log.read_payload(a.start, skip as u64, &mut buf))?;
            if buf != data[skip..skip + len] {
                return Err(format!("{len} bytes from {skip} read back wrong"));
            }
        }
        let mut buf = vec![0u8; 16];
        let refused = [
            log.read_payload(Lsn(a.start.0 + 64), 0, &mut buf),
            log.read_payload(a.start, 9_990, &mut buf),
            log.read_payload(a.start, u64::MAX, &mut buf),
            log.read_payload(small.start, 0, &mut buf),
            log.read_payload(a.end, 0, &mut buf),
            log.read_payload(Lsn(0), 0, &mut buf),
        ];
        if !refused
            .iter()
            .all(|r| matches!(r, Err(LogError::BadPosition(_))))
        {
            return Err(format!("bad reads gave {refused:?}"));
        }
        s(log.sync())?;
        s(log.reclaim(small.start))?;
        match log.read_payload(first.start, 0, &mut buf) {
            Err(LogError::BadPosition(_)) => {}
            other => return Err(format!("a read below the start gave {other:?}")),
        }
        disk.fail_write(0, false);
        if put_small(&mut log, 9).is_ok() {
            return Err("the failing append succeeded".into());
        }
        match log.read_payload(a.start, 0, &mut buf) {
            Err(LogError::Failed) => Ok(()),
            other => Err(format!("a read on a failed log gave {other:?}")),
        }
    }
    holds(scenario);
}

/// A log with a limit on its segment files is full when a record needs
/// one more. The append says so and has written nothing; the log has not
/// failed. After a reclaim has freed a file the same append works, in
/// the freed file, and the log goes on through a power cut.
#[test]
fn a_full_log_says_so_and_goes_on_after_a_reclaim() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        log.set_segment_limit(Some(2));
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        let mut want: Vec<u8> = first.iter().map(|(t, _)| *t).collect();
        // Fill the second file to the point where a record needs a third.
        let (tail, writes) = loop {
            let before = (log.tail(), disk.stats().writes);
            let data = vec![tag; env.payload()];
            match log.append(Kind::Write, u64::from(tag) * 4096, data.len() as u64, &data) {
                Ok(_) => {
                    want.push(tag);
                    tag += 1;
                }
                Err(LogError::Full) => break before,
                Err(other) => return Err(other.to_string()),
            }
        };
        if log.tail() != tail || disk.stats().writes != writes || log.is_failed() {
            return Err("the refused append changed the log".into());
        }
        if log.segment_files() != 2 {
            return Err(format!("{} segment files", log.segment_files()));
        }
        expect(&log, &want, "when full")?;
        // Full again, as often as it is tried.
        if put(&mut log, env, tag).is_ok() || log.is_failed() {
            return Err("a full log took a record or failed".into());
        }
        s(log.sync())?;
        let (_, kept) = first[first.len() - 1];
        s(log.reclaim(kept.start))?;
        want.drain(..first.len() - 1);
        put(&mut log, env, tag)?;
        want.push(tag);
        if log.segment_files() != 2 {
            return Err("room was made by a new file".into());
        }
        s(log.sync())?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after going on")
    }
    holds(scenario);
}

/// Create, interrupted: a power cut during creation leaves either no
/// log, and then creating again works, or a whole empty one.
#[test]
fn an_interrupted_creation_leaves_no_log_or_a_whole_one() {
    let env = environments(0)[0];
    let (mut none, mut whole) = (0, 0);
    for seed in 0..32 {
        let disk = SimDisk::new(seed);
        // Fail the directory sync that ends creation, or one before it.
        disk.fail_sync(seed % 4, false);
        let created = Log::create(disk.clone(), env.identity());
        assert!(created.is_err(), "seed {seed}");
        disk.power_cut(SeededCut::with_garbage(seed));
        match Log::open(disk.clone(), Options::default()) {
            Ok((log, recovery)) => {
                whole += 1;
                assert_eq!(recovery.end, recovery.start);
                assert_eq!(tags(&log).unwrap(), Vec::<u8>::new());
            }
            Err(LogError::NotFound) => {
                none += 1;
                let mut log = Log::create(disk.clone(), env.identity()).unwrap();
                put(&mut log, &env, 1).unwrap();
            }
            Err(other) => panic!("seed {seed}: a half-made log: {other}"),
        }
    }
    assert!(none > 0 && whole > 0, "none {none}, whole {whole}");
}

/// Process kill: every appended record is there afterwards, in order,
/// synced or not, across segments, and the log continues where it
/// ended.
#[test]
fn a_kill_keeps_every_appended_record() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let mut want: Vec<u8> = Vec::new();
        for _ in 0..2 {
            let filled = fill_to_roll(&mut log, env, &mut tag)?;
            want.extend(filled.iter().map(|(t, _)| *t));
        }
        let tail = log.tail();
        drop(log);
        let (mut log, recovery) = env.reopen(&disk)?;
        if recovery.end != tail || recovery.stopped != Stop::Header(Bad::Blank) {
            return Err(format!("recovery after a kill: {recovery:?}"));
        }
        if !recovery.checked_device || recovery.marker_found || recovery.follows.is_some() {
            return Err(format!("recovery after a kill: {recovery:?}"));
        }
        expect(&log, &want, "after a kill")?;
        if log.durable() != log.tail() || log.tail() < tail {
            return Err("recovery did not make the prefix durable".into());
        }
        put(&mut log, env, 99)?;
        want.push(99);
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after a second kill")
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// Power cuts: torn records, gaps, and things that must stay dead.

/// Torn record: a record that lost one sector of its payload is not
/// accepted. Mutant: a log that does not check the payload.
#[test]
fn a_torn_record_is_refused() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        drop(log);
        let last = body_sectors(b).pop().into_iter().collect::<Vec<_>>();
        env.cut(&disk, &last);
        let (log, recovery) = env.reopen(&disk)?;
        expect(&log, &[1], "after a tear")?;
        if recovery.end != b.start || recovery.stopped != Stop::Data {
            return Err(format!("the tear was reported as {recovery:?}"));
        }
        Ok(())
    }
    caught(scenario, Mutants::NO_DATA_CHECK, everywhere);
}

/// Gap: when a record is lost and the one after it survives whole, the
/// log ends before the lost one. Mutant: a recovery that skips the
/// damaged record and goes on.
#[test]
fn a_record_after_a_gap_is_refused() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        let c = put(&mut log, env, 3)?;
        drop(log);
        env.cut(&disk, &body_sectors(b));
        let (log, recovery) = env.reopen(&disk)?;
        expect(&log, &[1], "after a gap")?;
        // What follows the break is reported, and that is all.
        if recovery.follows != Some(c.start) {
            return Err(format!("the survivor was not reported: {recovery:?}"));
        }
        Ok(())
    }
    caught(scenario, Mutants::ACCEPT_AFTER_GAP, everywhere);
}

/// The history the stale-record tests start with: records 1, 2, 3, 4;
/// a power cut loses the payload of 2 and keeps 3 and 4; recovery ends
/// the log after 1.
fn lose_the_second(env: &Env) -> Result<(SimDisk, SimLog, [Appended; 4]), String> {
    let (disk, mut log) = env.fresh()?;
    let a = put(&mut log, env, 1)?;
    let b = put(&mut log, env, 2)?;
    let c = put(&mut log, env, 3)?;
    let d = put(&mut log, env, 4)?;
    drop(log);
    env.cut(&disk, &body_sectors(b));
    let (log, _) = env.reopen(&disk)?;
    expect(&log, &[1], "after the first power cut")?;
    Ok((disk, log, [a, b, c, d]))
}

/// Stale tail, the classic case: a record is lost and the ones after it
/// survive; after recovery new records are written, and the last of
/// them ends exactly where a survivor starts. The survivor must not
/// come back behind it. It is refused twice: its header does not follow
/// the header that is there now (the chain), and it is older than the
/// record before it (the epoch). Each alone is enough; without both it
/// comes back.
#[test]
fn the_chain_and_the_epoch_each_refuse_a_stale_record() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log, [_, _, c, d]) = lose_the_second(env)?;
        // Recovery sealed the end of the log, so new records start a
        // block later than the old ones did. Write until one ends where
        // an old record starts.
        let mut want = vec![1];
        let mut tag = 9;
        let landed = loop {
            let new = if env.align == 64 {
                put(&mut log, env, tag)?
            } else {
                put_small(&mut log, tag)?
            };
            want.push(tag);
            tag += 1;
            if new.end == c.start || new.end == d.start {
                break true;
            }
            if new.end > d.start {
                break false;
            }
        };
        if !landed {
            return Err("no new record ends where a stale one starts".into());
        }
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after new records")
    }
    covered(scenario, Mutants::NO_CHAIN);
    covered(scenario, Mutants::NO_EPOCH_CHECK);
    caught(
        scenario,
        Mutants::NO_CHAIN | Mutants::NO_EPOCH_CHECK,
        everywhere,
    );
}

/// Stale tail, the identical rewrite: record 2 is lost and 3 survives;
/// after recovery the very same record 2 is written again. Record 3
/// must not come back behind it. Two things prevent it. Recovery seals
/// the end of the log, so the rewrite does not land where the lost
/// record was. And recovery starts a new epoch, so that even there the
/// rewrite would be a different record, which the survivor does not
/// follow. Each alone is enough; a recovery that does neither brings
/// record 3 back.
#[test]
fn a_stale_record_stays_dead_behind_an_identical_rewrite() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log, _) = lose_the_second(env)?;
        put(&mut log, env, 2)?;
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[1, 2], "after the rewrite")
    }
    covered(scenario, Mutants::NO_EPOCH_BUMP);
    covered(scenario, Mutants::NO_TAIL_ERASE);
    caught(
        scenario,
        Mutants::NO_EPOCH_BUMP | Mutants::NO_TAIL_ERASE,
        everywhere,
    );
}

/// Stale tail, the sealed end. Power cut 1 loses the payload of A and
/// keeps A's header and the record R after it; recovery ends the log
/// before A. The client writes N, with A's payload. Power cut 2 loses
/// N's header sector, which then reads as it did before, and keeps N's
/// payload. Had N been written where A was, with A's header still
/// underneath, that header would be valid again, over a payload that
/// matches it, and R would follow it: a write the client never issued
/// in this history. So recovery seals the end of the prefix before it
/// returns: it writes a record of its own over what is there, durably.
/// Mutant: a recovery that does not.
#[test]
fn the_end_of_the_prefix_is_sealed() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let a = put(&mut log, env, 2)?;
        put(&mut log, env, 3)?;
        drop(log);
        env.cut(&disk, &body_sectors(a));
        let (mut log, _) = env.reopen(&disk)?;
        expect(&log, &[1], "after the first power cut")?;
        let n = put(&mut log, env, 2)?;
        if n.start <= a.start {
            return Err("the rewrite is over the seal".into());
        }
        drop(log);
        env.cut(&disk, &[header_sector(n)]);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[1], "after the second power cut")
    }
    caught(scenario, Mutants::NO_TAIL_ERASE, old_content);
}

/// The history the segment tests start with: a full segment and two
/// records in the next, none synced; a power cut loses the payload of
/// the last record of the first segment and keeps the second segment
/// whole; recovery ends the log in the first segment. Then that record
/// log goes on, rolls into the second segment and writes one record
/// there. Returns the tags that must be in the log before that record,
/// and the record.
fn stale_second_segment(env: &Env) -> Result<(SimDisk, SimLog, Vec<u8>, Appended), String> {
    let (disk, mut log) = env.fresh()?;
    let mut tag = 1;
    let first = fill_to_roll(&mut log, env, &mut tag)?;
    put(&mut log, env, 50)?;
    drop(log);
    let (_, last) = first[first.len() - 2];
    env.cut(&disk, &body_sectors(last));
    let (mut log, _) = env.reopen(&disk)?;
    let mut want: Vec<u8> = first[..first.len() - 2].iter().map(|(t, _)| *t).collect();
    expect(&log, &want, "after the first power cut")?;
    let mut tag = 60;
    let mut more = fill_to_roll(&mut log, env, &mut tag)?;
    let Some((_, w)) = more.pop() else {
        return Err("nothing was appended".into());
    };
    want.extend(more.iter().map(|(t, _)| *t));
    if w.start != Lsn(SEG + BLOCK) {
        return Err("the log did not roll where the test expects".into());
    }
    Ok((disk, log, want, w))
}

/// Stale first record of a segment. The chain starts afresh in every
/// segment, from a seed that depends only on the volume and the
/// segment's position, so a stale first record follows its seed as well
/// as a new one would. Here the new first record is lost at a second
/// power cut and the old one reads back in its place, under the
/// segment's new header. Only its epoch, older than the header's,
/// refuses it. Mutants: a log that does not check a record's epoch, and
/// a recovery that does not start a new one.
#[test]
fn a_stale_first_record_of_a_segment_is_refused() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, log, want, w) = stale_second_segment(env)?;
        drop(log);
        env.cut(&disk, &all_sectors(w));
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after the second power cut")
    }
    caught(scenario, Mutants::NO_EPOCH_CHECK, old_content);
    caught(scenario, Mutants::NO_EPOCH_BUMP, old_content);
}

/// Stale segment. After a recovery that ends in one segment, the file
/// that held the next part of the log still says so. Two things stop it
/// from being taken for the next segment later: recovery erases its
/// header, and a segment's header must not be older than the marker
/// that closed the segment before it. Here the second power cut loses
/// the new header of that file, so that it reads as it did before.
/// Each defence alone is enough; without both the stale segment and its
/// records come back.
#[test]
fn a_stale_segment_is_not_taken_for_the_next_one() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, log, want, w) = stale_second_segment(env)?;
        drop(log);
        let mut lose = all_sectors(w);
        lose.push((file_of(w.start), 0));
        env.cut(&disk, &lose);
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.stopped != Stop::NoNextSegment {
            return Err(format!("recovery stopped for {:?}", recovery.stopped));
        }
        expect(&log, &want, "after the second power cut")
    }
    covered(scenario, Mutants::KEEP_STALE_SEGMENTS);
    covered(scenario, Mutants::NO_SEGMENT_EPOCH_CHECK);
    caught(
        scenario,
        Mutants::KEEP_STALE_SEGMENTS | Mutants::NO_SEGMENT_EPOCH_CHECK,
        old_content,
    );
}

/// Two files, one part of the log. If a stale file still claims the
/// part that a newer file now holds, the newer one counts. (Recovery
/// erases stale claims, so this takes a log that does not, to show that
/// the preference alone is enough.) Mutant: a log that takes the older
/// claim, and so loses records a sync had covered.
#[test]
fn of_two_claims_to_one_segment_the_newer_counts() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        // Segment 0, and the first record of segment 1, synced.
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        let (_, f1) = first[first.len() - 1];
        s(log.sync())?;
        // The rest of segment 1 and two records of segment 2, not synced.
        let second = fill_to_roll(&mut log, env, &mut tag)?;
        put(&mut log, env, 70)?;
        // The log now starts in segment 1: the first file is free.
        if s(log.reclaim(f1.start))? != f1.start {
            return Err("reclaim did not reach the second segment".into());
        }
        drop(log);
        // Lose the last record of segment 1; segment 2 stays, stale.
        let (_, last) = second[second.len() - 2];
        env.cut(&disk, &body_sectors(last));
        let (mut log, _) = env.reopen(&disk)?;
        let mut want = vec![first[first.len() - 1].0];
        want.extend(second[..second.len() - 2].iter().map(|(t, _)| *t));
        expect(&log, &want, "after the power cut")?;
        // Roll into segment 2 again: it goes to the free first file.
        put(&mut log, env, 80)?;
        put(&mut log, env, 81)?;
        want.extend([80, 81]);
        s(log.sync())?;
        if log.segment_files() != 3 {
            return Err(format!("{} segment files", log.segment_files()));
        }
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after reopening with two claims")
    }
    covered(scenario, Mutants::KEEP_STALE_SEGMENTS);
    covered(scenario, Mutants::PREFER_OLDER_CLAIM);
    caught(
        scenario,
        Mutants::KEEP_STALE_SEGMENTS | Mutants::PREFER_OLDER_CLAIM,
        everywhere,
    );
}

/// A reused file. When a segment file is given a new part of the log,
/// the records of its earlier life are still in it, at the same offsets.
/// Each is refused three times: it does not follow the record before it
/// (the chain), it names a position in the part the file used to hold,
/// and it is older than the file's new header, because the reclaim that
/// freed the file started a new epoch. Each check alone is enough, also
/// any two of them missing; without all three, records from the earlier
/// life come back.
#[test]
fn the_position_and_the_chain_each_refuse_a_record_of_an_earlier_life() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        let (f1_tag, f1) = first[first.len() - 1];
        s(log.sync())?;
        s(log.reclaim(f1.start))?;
        // Fill segment 1; the roll into segment 2 reuses the first file.
        let second = fill_to_roll(&mut log, env, &mut tag)?;
        if log.segment_files() != 2 {
            return Err("the free file was not reused".into());
        }
        let mut want = vec![f1_tag];
        want.extend(second.iter().map(|(t, _)| *t));
        // The file's earlier life began with a recovery's mark, so its
        // records sit one header later than the new ones. One small
        // record brings the new life into step with the old.
        if env.align == 4096 {
            put_small(&mut log, 250)?;
            want.push(250);
        }
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after reusing a file")
    }
    covered(scenario, Mutants::NO_CHAIN | Mutants::NO_POSITION_CHECK);
    covered(scenario, Mutants::NO_CHAIN | Mutants::NO_EPOCH_CHECK);
    covered(
        scenario,
        Mutants::NO_POSITION_CHECK | Mutants::NO_EPOCH_CHECK,
    );
    caught(
        scenario,
        Mutants::NO_CHAIN | Mutants::NO_POSITION_CHECK | Mutants::NO_EPOCH_CHECK,
        everywhere,
    );
}

/// Contiguity: the log's segments follow one another without a hole. If
/// the middle one of three is not there, the log ends before it and
/// does not continue in the third.
#[test]
fn the_log_does_not_continue_past_a_missing_segment() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        fill_to_roll(&mut log, env, &mut tag)?;
        drop(log);
        // Damage the header of the second segment's file.
        disk.corrupt(&slot_name(1), 40, 0x01)
            .map_err(|e| e.to_string())?;
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.stopped != Stop::NoNextSegment {
            return Err(format!("recovery stopped for {:?}", recovery.stopped));
        }
        let want: Vec<u8> = first[..first.len() - 1].iter().map(|(t, _)| *t).collect();
        expect(&log, &want, "with the middle segment missing")
    }
    holds(scenario);
}

/// A payload is never taken for a record. A record appended directly
/// behind another can span the next unit boundary, and a walk looks at
/// that boundary before it looks directly behind a record. So the bytes
/// there are a client's, and the client may have made them a record
/// header: here a discard of some other part of the volume, correct in
/// every field, chained to the record before, with a valid tag under a
/// key that is not the log's. The walk must not take it; it must find
/// the real record. Mutant: a log whose headers are tagged with a key
/// everybody knows, which is what a plain chained checksum was.
#[test]
fn a_payload_that_looks_like_a_record_is_not_taken_for_one() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let before = put_small(&mut log, 1)?;
        let boundary = Lsn(before.end.0.div_ceil(BLOCK) * BLOCK);
        if boundary == before.end {
            // Records of this alignment start on boundaries only: no
            // payload lies where a walk looks for a header.
            return Ok(());
        }
        let payload_start = before.end.0 + u64::from(env.align);
        let at = (boundary.0 - payload_start) as usize;
        let mut payload = vec![7u8; at + 64 + 200];
        payload[at..at + 64].copy_from_slice(&log.lookalike_header(boundary, 77 * 4096));
        let victim = s(log.append(Kind::Write, 7 * 4096, 4096, &payload))?;
        if victim.start != before.end {
            return Err("the record is not directly behind the one before".into());
        }
        put_small(&mut log, 3)?;
        drop(log);
        let (log, _) = env.reopen(&disk)?;
        let mut seen = Vec::new();
        s(log.replay_unsynced(log.floor(), log.tail(), &mut |r| {
            seen.push((r.kind, r.offset, r.data.to_vec()));
        }))?;
        let want = vec![
            (Kind::Discard, 4096, vec![]),
            (Kind::Write, 7 * 4096, payload),
            (Kind::Discard, 3 * 4096, vec![]),
        ];
        if seen != want {
            let shape: Vec<_> = seen.iter().map(|(k, o, d)| (*k, *o, d.len())).collect();
            return Err(format!("the log holds {shape:?}"));
        }
        Ok(())
    }
    fn small_records(env: &Env) -> bool {
        env.align == 64
    }
    caught(scenario, Mutants::PUBLIC_KEY, small_records);
}

/// Recovery always seals, and never needs a free segment file to do it.
/// Forty recoveries in a row on a log whose last segment is nearly
/// full: each finds the same records, none adds a segment file, and
/// when the segment has no room left for a mark the seal is the marker
/// that closes it, after which a recovery writes no seal at all. The
/// log then goes on in the next segment.
#[test]
fn repeated_recoveries_on_a_full_log_find_the_same_prefix() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let mut want: Vec<u8> = Vec::new();
        for _ in 0..2 {
            let filled = fill_to_roll(&mut log, env, &mut tag)?;
            want.extend(filled.iter().map(|(t, _)| *t));
        }
        // Fill the last segment as far as records of this size go.
        let base = log.tail().0 / SEG;
        loop {
            s(log.sync())?;
            let record = put(&mut log, env, tag)?;
            if record.start.0 / SEG != base {
                break;
            }
            want.push(tag);
            tag += 1;
        }
        // That last record rolled into one more segment: lose it, so
        // that the log ends in the full one.
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        let held = tags(&log)?;
        if held != want {
            return Err(format!("after the power cut the log holds {held:?}"));
        }
        let files = log.segment_files();
        drop(log);
        let mut sealed_tails = Vec::new();
        for round in 0..40 {
            let (log, recovery) = env.reopen(&disk)?;
            if tags(&log)? != held {
                return Err(format!("round {round}: the log holds {:?}", tags(&log)?));
            }
            if log.segment_files() != files {
                return Err(format!("round {round}: recovery added a segment file"));
            }
            if log.tail().0 / SEG > recovery.end.0 / SEG {
                return Err(format!("round {round}: recovery left the segment"));
            }
            sealed_tails.push(log.tail());
        }
        // The seals stopped once the segment was closed.
        if sealed_tails[38] != sealed_tails[39] {
            return Err("recoveries went on writing seals in a closed segment".into());
        }
        let (mut log, _) = env.reopen(&disk)?;
        let next = put(&mut log, env, 240)?;
        if next.start.0 / SEG <= sealed_tails[39].0 / SEG {
            return Err("the log did not go on in the next segment".into());
        }
        s(log.sync())?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        let mut want = held;
        want.push(240);
        expect(&log, &want, "after going on")
    }
    holds(scenario);
}

/// A segment's header is written once for each use of its file: when
/// the file is given a part of the log, and never while it holds
/// records the log still needs. So nothing the log does later (appends,
/// syncs, a reclaim that stays within the segment, recoveries) puts a
/// header at risk: a power cut that turns every unsynced sector of
/// every header unit into garbage loses nothing.
#[test]
fn the_header_of_a_segment_in_use_is_never_written_again() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        s(log.sync())?;
        let mut want: Vec<u8> = first.iter().map(|(t, _)| *t).collect();
        // Everything the log does to segments that are in use.
        for _ in 0..3 {
            put(&mut log, env, tag)?;
            want.push(tag);
            tag += 1;
            s(log.sync())?;
        }
        let (_, second) = first[1];
        s(log.reclaim(second.start))?;
        want.remove(0);
        put(&mut log, env, tag)?;
        want.push(tag);
        s(log.sync())?;
        drop(log);
        let (mut log, _) = env.reopen(&disk)?;
        put(&mut log, env, 200)?;
        drop(log);
        disk.power_cut(BySector(|_: Option<&str>, index| {
            if index < BLOCK / SECTOR {
                Fate::Garbage
            } else {
                Fate::Latest
            }
        }));
        let (log, _) = env.reopen(&disk)?;
        want.push(200);
        expect(&log, &want, "after garbage in every header unit")
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// Sync.

/// Sync: what a successful sync covered survives a power cut that loses
/// everything it can. Mutant: a sync that returns without syncing.
#[test]
fn a_sync_covers_every_record_before_it() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        if log.durable() >= b.end {
            return Err("durable before any sync".into());
        }
        if s(log.sync())? != b.end || log.durable() != b.end {
            return Err("sync did not report the tail as durable".into());
        }
        put(&mut log, env, 3)?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[1, 2], "after a power cut")
    }
    caught(scenario, Mutants::SYNC_EARLY, everywhere);
}

/// A block that holds durable records is not written again. Small
/// records share a block, and a block that is being written when the
/// power fails can come back as garbage. So after a sync, and after a
/// recovery, the next record starts a new block: here a power cut turns
/// every sector the later record touched into garbage, and what was
/// durable before it is still there. Mutant: a log that goes on in the
/// same block.
#[test]
fn a_block_with_durable_records_is_not_written_again() {
    fn scenario(env: &Env) -> Outcome {
        let garbage_all = |disk: &SimDisk| {
            let lost = env.lost();
            disk.power_cut(BySector(move |_: Option<&str>, _| lost));
        };
        let (disk, mut log) = env.fresh()?;
        put_small(&mut log, 1)?;
        let synced = s(log.sync())?;
        let after = put_small(&mut log, 2)?;
        if after.start.0 / BLOCK == (synced.0 - 1) / BLOCK && env.mutants == Mutants::default() {
            return Err("a record went into a block that holds durable records".into());
        }
        drop(log);
        garbage_all(&disk);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[1], "after a power cut behind a sync")?;
        drop(log);
        // The same after a recovery: a kill, then a record, then the cut.
        let (mut log, _) = env.reopen(&disk)?;
        put_small(&mut log, 3)?;
        drop(log);
        let (mut log, _) = env.reopen(&disk)?;
        expect(&log, &[1, 3], "after a kill")?;
        put_small(&mut log, 4)?;
        drop(log);
        garbage_all(&disk);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &[1, 3], "after a power cut behind a recovery")
    }
    fn small_records_and_garbage(env: &Env) -> bool {
        env.align == 64 && env.garbage
    }
    caught(
        scenario,
        Mutants::REWRITE_DURABLE_BLOCK,
        small_records_and_garbage,
    );
}

/// The same rule, from the moment a sync is issued, not from its
/// completion: a record appended while a sync is in flight would dirty
/// the unit again after the sync has made it durable. The log is serial
/// today, so the test looks at the only sync that does not complete,
/// one that fails: already then the next record would start a new unit.
#[test]
fn a_sync_reserves_its_last_unit_when_it_is_issued() {
    for env in environments(0) {
        let (disk, mut log) = env.fresh().unwrap();
        put_small(&mut log, 1).unwrap();
        let tail = log.tail();
        assert_eq!(log.next_start(), tail, "{env:?}");
        disk.fail_sync(0, false);
        assert!(log.sync().is_err());
        assert_eq!(log.tail(), tail);
        assert_eq!(log.next_start().0 % BLOCK, 0, "{env:?}");
        assert!(log.next_start() >= tail);
        if env.align == 64 {
            assert!(log.next_start() > tail);
        }
        // A sync with nothing to cover reserves nothing.
        let (_disk, mut log) = env.fresh().unwrap();
        put_small(&mut log, 1).unwrap();
        log.sync().unwrap();
        let after = put_small(&mut log, 2).unwrap();
        log.sync().unwrap();
        log.sync().unwrap();
        let next = put_small(&mut log, 3).unwrap();
        assert_eq!(next.start.0, after.end.0.div_ceil(BLOCK) * BLOCK, "{env:?}");
    }
}

/// Group commit: one sync covers every append before it, with one
/// device sync per segment file that has unsynced records, and a sync
/// with nothing to do does none.
#[test]
fn one_sync_covers_many_appends_and_an_idle_sync_is_free() {
    let env = environments(0)[0];
    let (disk, mut log) = env.fresh().unwrap();
    for tag in 1..=5 {
        put(&mut log, &env, tag).unwrap();
    }
    let before = disk.stats();
    log.sync().unwrap();
    assert_eq!(disk.stats().syncs, before.syncs + 1);
    assert_eq!(disk.stats().dir_syncs, before.dir_syncs);
    log.sync().unwrap();
    log.sync().unwrap();
    assert_eq!(disk.stats().syncs, before.syncs + 1);
}

/// Sync across a segment switch: a sync that covers records in two
/// segments syncs both files, not only the one being written. Mutant: a
/// sync of the current segment only.
#[test]
fn a_sync_covers_every_segment_with_unsynced_records() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        s(log.sync())?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        let want: Vec<u8> = filled.iter().map(|(t, _)| *t).collect();
        expect(&log, &want, "after a power cut")
    }
    caught(scenario, Mutants::SYNC_ONLY_TAIL, everywhere);
}

/// A power cut that keeps every sector and loses every name that no
/// directory sync covered.
struct LoseNames;

impl Cut for LoseNames {
    fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
        Fate::Latest
    }
    fn name(&mut self, _: usize, _: usize) -> bool {
        false
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        true
    }
}

/// Sync across a segment switch, the new file: the durable position
/// moves past records in a new segment file only once that file's name
/// is durable too. Before the sync it has not moved; after it, a power
/// cut that loses every unsynced name loses nothing. Mutant: a sync
/// that does not sync the directory.
#[test]
fn a_sync_does_not_claim_records_in_a_file_that_could_vanish() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        let (_, in_new_file) = filled[filled.len() - 1];
        if log.durable() >= in_new_file.start {
            return Err("records in a new file were durable before any sync".into());
        }
        let claimed = s(log.sync())?;
        if claimed != in_new_file.end {
            return Err("the sync did not cover the new segment".into());
        }
        drop(log);
        disk.power_cut(LoseNames);
        let (log, _) = env.reopen(&disk)?;
        let want: Vec<u8> = filled.iter().map(|(t, _)| *t).collect();
        expect(&log, &want, "after losing unsynced names")
    }
    caught(scenario, Mutants::SYNC_NO_DIR, everywhere);
}

/// The new file, without a sync: records in a segment file whose name
/// is lost at a power cut are lost with it, and the log is the prefix
/// before them. They were never claimed.
#[test]
fn records_in_a_vanished_file_are_a_clean_loss() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        drop(log);
        disk.power_cut(LoseNames);
        let (mut log, recovery) = env.reopen(&disk)?;
        if recovery.stopped != Stop::NoNextSegment {
            return Err(format!("recovery stopped for {:?}", recovery.stopped));
        }
        let mut want: Vec<u8> = filled[..filled.len() - 1].iter().map(|(t, _)| *t).collect();
        expect(&log, &want, "after the file vanished")?;
        // And the log goes on: it rolls again, into a new file.
        put(&mut log, env, 90)?;
        want.push(90);
        s(log.sync())?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "after rolling again")
    }
    holds(scenario);
}

/// A failed sync is final. After a sync has failed, the data it did not
/// write may be gone from the cache as well, so no later sync can vouch
/// for it: the log fails, and never again reports a position as
/// durable. Mutant: a log that tries the sync again and believes it.
#[test]
fn a_failed_sync_is_never_retried() {
    fn scenario(env: &Env) -> Outcome {
        for seed in 0..12 {
            let env = Env { seed, ..*env };
            let (disk, mut log) = env.fresh()?;
            let a = put(&mut log, &env, 1)?;
            s(log.sync())?;
            put(&mut log, &env, 2)?;
            put(&mut log, &env, 3)?;
            disk.fail_sync(0, false);
            if log.sync().is_ok() {
                return Err("a failing sync reported success".into());
            }
            let claimed = log.sync().unwrap_or(a.end);
            drop(log);
            disk.power_cut(LoseAll);
            let (log, recovery) = env.reopen_ack(&disk)?;
            if recovery.end < claimed {
                return Err(format!(
                    "seed {seed}: a sync claimed {claimed}, the log ends at {}",
                    recovery.end
                ));
            }
            let got = tags(&log)?;
            if !got.starts_with(&[1]) || !([1, 2, 3].starts_with(&got)) {
                return Err(format!("seed {seed}: the log holds {got:?}"));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::RETRY_SYNC, everywhere);
}

/// A failed log shows no records. After a failed sync the cache may
/// hold records the disk does not, and they pass every check when read
/// through the cache; someone who replayed them into a place that
/// outlives the log would keep what the log itself loses at the next
/// power cut. So a replay on a failed log is refused, like an append,
/// a sync and a reclaim, and the positions stay readable. Mutant: a
/// failed log that still replays.
#[test]
fn a_failed_log_replays_nothing() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        s(log.sync())?;
        let durable = log.durable();
        put(&mut log, env, 2)?;
        disk.fail_sync(0, false);
        if log.sync().is_ok() {
            return Err("a failing sync reported success".into());
        }
        let mut seen = 0;
        match log.replay_unsynced(log.floor(), log.tail(), &mut |_| seen += 1) {
            Err(LogError::Failed) if seen == 0 => {}
            other => return Err(format!("replay gave {other:?} and {seen} records")),
        }
        if !log.is_failed() || log.durable() != durable {
            return Err("the failed log's positions changed".into());
        }
        Ok(())
    }
    caught(scenario, Mutants::REPLAY_WHEN_FAILED, everywhere);
}

// ---------------------------------------------------------------------
// Recovery makes what it returns durable.

/// Recovery validates what the device holds. After a failed sync the
/// cache can show records that never reached the disk, also to the next
/// process, whose own sync then succeeds without writing them. A
/// recovery that read through the cache would return them as durable,
/// and the next power cut would take them back. So: whatever recovery
/// returns is still there after a power cut straight after it. Mutant: a
/// recovery that validates through the cache.
#[test]
fn recovery_validates_what_the_device_holds() {
    fn scenario(env: &Env) -> Outcome {
        for seed in 0..12 {
            let env = Env { seed, ..*env };
            let (disk, mut log) = env.fresh()?;
            put(&mut log, &env, 1)?;
            s(log.sync())?;
            // Records of one sector each (at alignment 64, several in
            // one): the failed sync leaves some of them whole in the
            // cache and absent from the disk.
            for tag in 2..=9 {
                put_small(&mut log, tag)?;
            }
            // The sync fails, and so does recording the failure.
            disk.fail_sync(0, true);
            match log.sync() {
                Err(LogError::Io {
                    marker_written: false,
                    ..
                }) => {}
                other => return Err(format!("the failing sync returned {other:?}")),
            }
            disk.clear_faults();
            drop(log);
            // The marker's file was created and could not be synced. Take
            // the worse case, that it could not be created at all: the
            // new instance then has nothing to warn it.
            if disk
                .list()
                .is_ok_and(|names| names.iter().any(|n| n == MARKER_NAME))
            {
                disk.remove(MARKER_NAME).map_err(|e| e.to_string())?;
            }
            let (log, first) = env.reopen(&disk)?;
            let held = tags(&log)?;
            drop(log);
            disk.power_cut(LoseAll);
            let (log, second) = env.reopen(&disk)?;
            if second.end < first.end || tags(&log)? != held {
                return Err(format!(
                    "seed {seed}: recovery returned {held:?} up to {}, and a power cut \
                     left {:?} up to {}",
                    first.end,
                    tags(&log)?,
                    second.end
                ));
            }
            if !first.checked_device {
                return Err("recovery did not say that it read from the device".into());
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::VALIDATE_CACHED, everywhere);
}

/// Recovery syncs before it validates. After a plain kill, records are
/// with the operating system and not on the disk. Recovery returns them
/// (a kill loses nothing), so it must first make them durable: a power
/// cut straight after recovery changes nothing. Mutant: a recovery that
/// does not sync first.
#[test]
fn recovery_makes_the_prefix_durable() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        drop(log);
        let (log, first) = env.reopen(&disk)?;
        let want: Vec<u8> = filled.iter().map(|(t, _)| *t).collect();
        expect(&log, &want, "after a kill")?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, second) = env.reopen(&disk)?;
        if second.end < first.end {
            return Err(format!(
                "recovery returned {}, a power cut left {}",
                first.end, second.end
            ));
        }
        expect(&log, &want, "after a power cut straight after recovery")
    }
    caught(scenario, Mutants::NO_SYNC_BEFORE_VALIDATE, everywhere);
}

/// No device reads: on a file system without them recovery still works,
/// through the cache, and says that it could not check.
#[test]
fn recovery_reports_when_it_cannot_read_the_device() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        drop(log);
        disk.set_device_reads(false);
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.checked_device {
            return Err("recovery claims a check it could not make".into());
        }
        expect(&log, &[1], "without device reads")
    }
    holds(scenario);
}

/// Recovery, twice: recovering, cutting the power at once and
/// recovering again gives the same log, whatever the first power cut
/// did to the unsynced tail.
#[test]
fn recovering_twice_gives_the_same_prefix() {
    fn scenario(env: &Env) -> Outcome {
        for seed in 0..24 {
            let (disk, mut log) = env.fresh()?;
            let mut tag = 1;
            fill_to_roll(&mut log, env, &mut tag)?;
            s(log.sync())?;
            let synced = tags(&log)?;
            for _ in 0..6 {
                put(&mut log, env, tag)?;
                tag += 1;
            }
            let all = tags(&log)?;
            drop(log);
            if env.garbage {
                disk.power_cut(SeededCut::with_garbage(seed));
            } else {
                disk.power_cut(SeededCut::new(seed));
            }
            let (log, first) = env.reopen(&disk)?;
            let held = tags(&log)?;
            if !held.starts_with(&synced) || !all.starts_with(&held) {
                return Err(format!("seed {seed}: not a prefix: {held:?}"));
            }
            drop(log);
            disk.power_cut(LoseAll);
            let (log, second) = env.reopen(&disk)?;
            if second.end < first.end || tags(&log)? != held {
                return Err(format!("seed {seed}: the second recovery differs"));
            }
        }
        Ok(())
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// Failure.

/// A failed log stays failed. After a write error nothing succeeds, the
/// failure is recorded in a marker, and a new instance refuses to open
/// the log until the caller acknowledges the failure. Then the log
/// holds at least what every successful sync covered, and the marker is
/// gone. Mutant: a log that writes no marker.
#[test]
fn a_failed_log_stays_failed_across_a_reopen() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        s(log.sync())?;
        put(&mut log, env, 2)?;
        disk.fail_write(0, false);
        match put_raw(&mut log, env, 3) {
            Err(LogError::Io {
                marker_written: true,
                ..
            }) => {}
            other => return Err(format!("the failing append returned {other:?}")),
        }
        if !log.is_failed() {
            return Err("the log does not know it failed".into());
        }
        let after = [
            put_raw(&mut log, env, 4).err(),
            log.sync().err(),
            log.reclaim(log.tail()).err(),
        ];
        if after != [Some(LogError::Failed); 3] {
            return Err(format!("after the failure: {after:?}"));
        }
        drop(log);
        disk.kill();
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::MarkedFailed) => {}
            Err(other) => return Err(other.to_string()),
            Ok(_) => return Err("a new instance opened the failed log unasked".into()),
        }
        let (log, recovery) = env.reopen_ack(&disk)?;
        if !recovery.marker_found {
            return Err("the marker was not reported".into());
        }
        let got = tags(&log)?;
        if !got.starts_with(&[1]) || !([1, 2, 3].starts_with(&got)) {
            return Err(format!("after the failure the log holds {got:?}"));
        }
        drop(log);
        let (_, recovery) = env.reopen(&disk)?;
        if recovery.marker_found {
            return Err("the marker outlived its acknowledgement".into());
        }
        Ok(())
    }
    caught(scenario, Mutants::NO_MARKER, everywhere);
}

fn put_raw(log: &mut SimLog, env: &Env, tag: u8) -> Result<Appended, LogError> {
    log.append(
        Kind::Write,
        u64::from(tag) * 4096,
        1,
        &vec![tag; env.payload()],
    )
}

/// The gap in the marker: when the disk refuses the marker as well, the
/// failing call says so, and a new instance cannot know. The log still
/// holds what every successful sync covered. (That a client is not
/// served by such an instance is the transport's duty: design §4.1.)
#[test]
fn a_failure_that_cannot_be_recorded_is_reported_as_such() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        s(log.sync())?;
        disk.fail_write(0, true);
        disk.fail_sync(0, true);
        match put_raw(&mut log, env, 2) {
            Err(LogError::Io {
                marker_written: false,
                ..
            }) => {}
            other => return Err(format!("the failing append returned {other:?}")),
        }
        drop(log);
        disk.power_cut(LoseAll);
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.marker_found {
            return Err("a marker that could not be written was found".into());
        }
        expect(&log, &[1], "after an unrecorded failure")
    }
    holds(scenario);
}

/// Order of acknowledgements: a record is acknowledged only when its
/// write and every earlier write have returned. So when a write fails
/// part-way, nothing after it is ever acknowledged, and what a new
/// instance finds is the acknowledged records, in order, with at most
/// the failed one behind them.
#[test]
fn nothing_is_acknowledged_behind_a_failed_write() {
    fn scenario(env: &Env) -> Outcome {
        let mut with_failed = 0;
        for seed in 0..12 {
            let env = Env { seed, ..*env };
            let (disk, mut log) = env.fresh()?;
            put(&mut log, &env, 1)?;
            put(&mut log, &env, 2)?;
            disk.fail_write(0, false);
            if put(&mut log, &env, 3).is_ok() || put(&mut log, &env, 4).is_ok() {
                return Err("an append succeeded behind a failed write".into());
            }
            drop(log);
            let (log, _) = env.reopen_ack(&disk)?;
            match tags(&log)?.as_slice() {
                [1, 2] => {}
                [1, 2, 3] => with_failed += 1,
                other => return Err(format!("seed {seed}: the log holds {other:?}")),
            }
        }
        // A write that fails may have written everything.
        let _ = with_failed;
        Ok(())
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// Reclaim.

/// The hold: reclaim never passes it, and never passes the durable
/// position; positions before the new start are no longer served; and a
/// reopened log starts where reclaim left it. Mutant: a reclaim that
/// ignores the hold.
#[test]
fn reclaim_stops_at_the_hold_and_at_the_durable_position() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let start = log.floor();
        let mut tag = 1;
        let mut all = fill_to_roll(&mut log, env, &mut tag)?;
        all.extend(fill_to_roll(&mut log, env, &mut tag)?);
        let durable = s(log.sync())?;
        let unsynced = put(&mut log, env, 100)?;
        let (_, held) = all[3];
        log.set_hold(Some(held.start));
        if log.hold() != Some(held.start) {
            return Err("the hold was not kept".into());
        }
        if s(log.reclaim(log.tail()))? != held.start || log.floor() != held.start {
            return Err(format!(
                "reclaim passed the hold: the log starts at {}",
                log.floor()
            ));
        }
        let mut want: Vec<u8> = all[3..].iter().map(|(t, _)| *t).collect();
        want.push(100);
        expect(&log, &want, "after reclaiming up to the hold")?;
        if !matches!(
            log.replay_unsynced(start, log.tail(), &mut |_| {}),
            Err(LogError::BadPosition(_))
        ) {
            return Err("a reclaimed position was served".into());
        }
        // Nothing to do is not an error, and the start never moves back.
        if s(log.reclaim(start))? != held.start {
            return Err("reclaim moved the start back".into());
        }
        log.set_hold(None);
        if s(log.reclaim(unsynced.end))? != durable {
            return Err("reclaim passed the durable position".into());
        }
        drop(log);
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.start != durable {
            return Err(format!("the reopened log starts at {}", recovery.start));
        }
        expect(&log, &[100], "after reopening")
    }
    caught(scenario, Mutants::IGNORE_HOLD, everywhere);
}

/// Reclaim, power cut before the space is reused: once reclaim has
/// returned, the new start is durable. A power cut straight after
/// leaves a log that starts there.
#[test]
fn a_power_cut_after_reclaim_keeps_the_new_start() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        let (f1_tag, f1) = first[first.len() - 1];
        s(log.sync())?;
        s(log.reclaim(f1.start))?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.start != f1.start {
            return Err(format!("the log starts at {}", recovery.start));
        }
        expect(&log, &[f1_tag], "after the power cut")
    }
    holds(scenario);
}

/// Reclaim, power cut after the space is reused: a segment file is
/// given a new part of the log only after the start that excludes its
/// old part is durable. Otherwise a power cut could leave a log whose
/// start lies in a segment that has been overwritten. Mutant: a reclaim
/// that frees the file before the new start is durable.
#[test]
fn a_segment_is_reused_only_after_the_new_start_is_durable() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let first = fill_to_roll(&mut log, env, &mut tag)?;
        let (f1_tag, f1) = first[first.len() - 1];
        s(log.sync())?;
        s(log.reclaim(f1.start))?;
        let second = fill_to_roll(&mut log, env, &mut tag)?;
        s(log.sync())?;
        if log.segment_files() != 2 {
            return Err("the free file was not reused".into());
        }
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        let mut want = vec![f1_tag];
        want.extend(second.iter().map(|(t, _)| *t));
        expect(&log, &want, "after the power cut")
    }
    caught(scenario, Mutants::REUSE_BEFORE_FLOOR_SYNC, everywhere);
}

// ---------------------------------------------------------------------
// The superblock.

/// The superblock is never overwritten in place: an update goes to the
/// copy that does not hold the current one, so an update torn by a
/// power cut leaves the current one. Here the sync of an update fails
/// and the power cut turns the sector that was being written into
/// garbage. Mutant: a log that always writes the same copy.
#[test]
fn a_torn_superblock_update_leaves_the_previous_one() {
    fn scenario(env: &Env) -> Outcome {
        for seed in 0..8 {
            let env = Env { seed, ..*env };
            let (disk, mut log) = env.fresh()?;
            let mut tag = 1;
            let first = fill_to_roll(&mut log, &env, &mut tag)?;
            let (_, f1) = first[first.len() - 1];
            s(log.sync())?;
            let all = tags(&log)?;
            disk.fail_sync(0, false);
            if log.reclaim(f1.start).is_ok() {
                return Err("a reclaim whose sync failed succeeded".into());
            }
            // The log has failed like after any failed write or sync,
            // and gave no segment file free.
            if !log.is_failed() || put(&mut log, &env, 200).is_ok() || log.sync().is_ok() {
                return Err("the log went on after a failed reclaim".into());
            }
            drop(log);
            let lost = env.lost();
            disk.power_cut(BySector(|name: Option<&str>, _| {
                if name == Some(SUPER_NAME) {
                    lost
                } else {
                    Fate::Latest
                }
            }));
            let (log, _) = env.reopen_ack(&disk)?;
            let got = tags(&log)?;
            if got != all && got != all[all.len() - 1..] {
                return Err(format!("seed {seed}: the log holds {got:?}"));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::ONE_SUPER_COPY, garbage);
}

/// A power cut during recovery's own superblock update: the epoch bump
/// is torn, the next recovery finds the previous superblock and the
/// same log, and bumps again.
#[test]
fn a_power_cut_during_recovery_is_recovered_from() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        put(&mut log, env, 2)?;
        drop(log);
        disk.kill();
        // Recovery syncs the superblock file, the segment and the
        // directory, and then its superblock update: fail that one.
        disk.fail_sync(3, false);
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Io {
                during: "writing the superblock",
                ..
            }) => {}
            Err(other) => return Err(format!("the test's count of syncs is off: {other}")),
            Ok(_) => return Err("the test's count of syncs is off".into()),
        }
        let lost = env.lost();
        disk.power_cut(BySector(|name: Option<&str>, _| {
            if name == Some(SUPER_NAME) {
                lost
            } else {
                Fate::Latest
            }
        }));
        let (log, _) = env.reopen_ack(&disk)?;
        expect(&log, &[1, 2], "after the interrupted recovery")
    }
    holds(scenario);
}

// ---------------------------------------------------------------------
// Corruption.

/// Corruption in the middle of the log, above the durable floor: a
/// flipped bit in a record ends the log before that record, the reason
/// and the records that follow are reported, and nothing behind the
/// break is returned.
#[test]
fn a_flipped_bit_ends_the_log_there() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        let c = put(&mut log, env, 3)?;
        s(log.sync())?;
        drop(log);
        let inside = b.start.0 + u64::from(env.align) + 100;
        disk.corrupt(&slot_name(0), inside, 0x10)
            .map_err(|e| e.to_string())?;
        let (log, recovery) = env.reopen(&disk)?;
        if recovery.end != b.start
            || recovery.stopped != Stop::Data
            || recovery.follows != Some(c.start)
        {
            return Err(format!("the corruption was reported as {recovery:?}"));
        }
        expect(&log, &[1], "after the corruption")
    }
    holds(scenario);
}

/// Corruption below the durable floor: the superblock records a
/// position that was durable. A log that ends before it has lost
/// something a sync had covered: that is an error, not a shorter log.
/// Mutant: a recovery that does not look at the floor.
#[test]
fn a_log_that_ends_below_the_durable_floor_is_corrupt() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        let c = put(&mut log, env, 3)?;
        drop(log);
        // This recovery records the end of the log as durable.
        drop(env.reopen(&disk)?);
        let inside = b.start.0 + u64::from(env.align) + 100;
        disk.corrupt(&slot_name(0), inside, 0x10)
            .map_err(|e| e.to_string())?;
        disk.kill();
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Corrupt(Corruption::BelowDurableFloor {
                end,
                durable_floor,
                stopped: Stop::Data,
            })) if end == b.start && durable_floor == c.end => Ok(()),
            Err(other) => Err(other.to_string()),
            Ok(_) => Err("a log that lost synced records opened as a shorter log".into()),
        }
    }
    caught(scenario, Mutants::NO_DURABLE_FLOOR, everywhere);
}

/// Corruption found by a replay: every record is checked again when it
/// is read, and a damaged one is an error. The visitor never sees it.
#[test]
fn a_replay_never_returns_bad_data() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        let b = put(&mut log, env, 2)?;
        put(&mut log, env, 3)?;
        let inside = b.start.0 + u64::from(env.align) + 100;
        disk.corrupt(&slot_name(0), inside, 0x10)
            .map_err(|e| e.to_string())?;
        let mut seen = Vec::new();
        let result = log.replay_unsynced(log.floor(), log.tail(), &mut |r| seen.push(r.data[0]));
        let want = Err(LogError::Corrupt(Corruption::Record {
            position: b.start,
            stopped: Stop::Data,
        }));
        if result != want || seen != [1] {
            return Err(format!("the replay returned {result:?} after {seen:?}"));
        }
        Ok(())
    }
    holds(scenario);
}

/// A read error is never the end of the log. Recovery treats the place
/// where no valid record follows as the end, for good: it starts a new
/// epoch, seals, and erases the segments behind. A read that fails says
/// nothing about what is there, so an open that meets one returns the
/// error and has written nothing: the disk is byte for byte as it was.
/// The error is placed at every read an open makes, in turn, through
/// the cache and from the device; after each, an undisturbed open finds
/// every record. Mutant: a walk that takes a read error for the end.
#[test]
fn a_read_error_never_shortens_the_log() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let mut want: Vec<u8> = Vec::new();
        for _ in 0..2 {
            let filled = fill_to_roll(&mut log, env, &mut tag)?;
            want.extend(filled.iter().map(|(t, _)| *t));
        }
        s(log.sync())?;
        drop(log);
        for device in [true, false] {
            disk.set_device_reads(device);
            let mut struck = 0;
            for after in 0.. {
                disk.kill();
                let image = disk.image();
                let writes = disk.stats().writes;
                disk.fail_read(after, false);
                match Log::open(disk.clone(), env.options()) {
                    Err(LogError::Io {
                        marker_written: false,
                        ..
                    }) => {
                        struck += 1;
                        if disk.stats().writes != writes || disk.image() != image {
                            return Err(format!(
                                "read {after}: the open failed and had changed the disk"
                            ));
                        }
                    }
                    Err(other) => return Err(format!("read {after}: {other}")),
                    Ok((log, _)) => {
                        // The error was armed: did it strike in the open?
                        let mut probe = [0u8; 1];
                        let unused = disk
                            .open(SUPER_NAME)
                            .is_ok_and(|file| file.read_at(0, &mut probe).is_err());
                        expect(&log, &want, &format!("with read {after} failing"))?;
                        if unused {
                            // Every read of an open has had its turn.
                            break;
                        }
                    }
                }
                disk.clear_faults();
            }
            if struck < 3 {
                return Err(format!("only {struck} reads of an open could fail"));
            }
        }
        disk.set_device_reads(true);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "in the end")
    }
    caught(scenario, Mutants::READ_ERROR_ENDS_LOG, everywhere);
}

/// The failure marker is durable when the failing call says it was
/// written: a power cut straight after the failure does not take the
/// marker's name away, and the next open refuses the log. Mutant: a
/// marker whose name is not synced.
#[test]
fn the_failure_marker_survives_a_power_cut() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        s(log.sync())?;
        disk.fail_write(0, false);
        match log.append(Kind::Discard, 0, 4096, &[]) {
            Err(LogError::Io {
                marker_written: true,
                ..
            }) => {}
            other => return Err(format!("the failing append returned {other:?}")),
        }
        drop(log);
        disk.power_cut(LoseAll);
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::MarkedFailed) => {}
            Err(other) => return Err(other.to_string()),
            Ok(_) => return Err("the log opened as if it had never failed".into()),
        }
        let (log, recovery) = env.reopen_ack(&disk)?;
        if !recovery.marker_found {
            return Err("recovery did not report the marker".into());
        }
        expect(&log, &[1], "after the acknowledged failure")
    }
    caught(scenario, Mutants::MARKER_UNSYNCED, everywhere);
}

/// A segment header of a later epoch than the superblock's means the
/// newest superblock copy is lost, like a record of a later epoch, and
/// is reported at the segment, before any record is looked at. Here a
/// record of the later epoch lies before the segment and would be
/// reported in its place; after reclaims the header can be the only
/// witness (the next test). Mutant: a log without the check on headers,
/// which reports the record.
#[test]
fn a_segment_newer_than_the_superblock_is_noticed() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        drop(log);
        // A recovery, and in its epoch a new segment.
        let (mut log, _) = env.reopen(&disk)?;
        let mut tag = 2;
        let filled = fill_to_roll(&mut log, env, &mut tag)?;
        s(log.sync())?;
        let second = filled[filled.len() - 1].1.start.0 / SEG * SEG;
        drop(log);
        disk.kill();
        disk.corrupt(
            SUPER_NAME,
            current_copy(&disk) * SUPER_COPY_STRIDE + 70,
            0x01,
        )
        .map_err(|e| e.to_string())?;
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Corrupt(Corruption::SuperblockBehind { position, .. }))
                if position == Lsn(second) =>
            {
                Ok(())
            }
            Err(other) => Err(other.to_string()),
            Ok(_) => Err("the log opened with a stale superblock".into()),
        }
    }
    caught(scenario, Mutants::NO_HEADER_FUTURE_CHECK, everywhere);
}

/// The reviewer's sequence: fill segments 0 and S and start 2S;
/// reclaim to S, then to 2S; go on into 3S and 4S, which reuse the
/// files of 0 and S; reclaim to 4S; go on into 5S, in the file that
/// held 3S; sync. Returns the disk and the tags from 4S on.
fn reclaimed_twice_over(env: &Env) -> Result<(SimDisk, Vec<u8>), String> {
    let (disk, mut log) = env.fresh()?;
    let mut tag = 1;
    let start_of = |filled: &[(u8, Appended)]| filled[filled.len() - 1].1.start;
    let a = fill_to_roll(&mut log, env, &mut tag)?;
    let b = fill_to_roll(&mut log, env, &mut tag)?;
    s(log.sync())?;
    s(log.reclaim(start_of(&a)))?;
    s(log.reclaim(start_of(&b)))?;
    fill_to_roll(&mut log, env, &mut tag)?;
    let d = fill_to_roll(&mut log, env, &mut tag)?;
    s(log.sync())?;
    s(log.reclaim(start_of(&d)))?;
    let e = fill_to_roll(&mut log, env, &mut tag)?;
    s(log.sync())?;
    if log.segment_files() != 3 {
        return Err(format!("{} segment files", log.segment_files()));
    }
    let mut want = vec![d[d.len() - 1].0];
    want.extend(e.iter().map(|(t, _)| *t));
    drop(log);
    disk.kill();
    Ok((disk, want))
}

/// A superblock copy that is lost after reclaims. Every superblock that
/// is written starts a new epoch, a reclaim's too. So whatever was
/// written after the newest copy, a record or the header of a segment
/// file that a reclaim freed, is of an epoch the older copy does not
/// have, and an open that finds only the older copy fails: it does not
/// take the older, shorter log that copy describes, and it does not
/// erase the segments of the real one. The disk is as it was, and with
/// the copy back the log opens whole. Here the only witness is the
/// header of a segment that does not lie on the older copy's path.
/// Mutants: a reclaim that keeps the epoch, and a log that does not
/// check the epochs of segment headers.
#[test]
fn a_lost_superblock_copy_after_reclaims_is_noticed() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, want) = reclaimed_twice_over(env)?;
        let at = current_copy(&disk) * SUPER_COPY_STRIDE + 70;
        disk.corrupt(SUPER_NAME, at, 0x01)
            .map_err(|e| e.to_string())?;
        let image = disk.image();
        let writes = disk.stats().writes;
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Corrupt(Corruption::SuperblockBehind { .. })) => {}
            Err(other) => return Err(other.to_string()),
            Ok((log, _)) => {
                return Err(format!(
                    "the log opened with a stale superblock and holds {:?}",
                    tags(&log)
                ))
            }
        }
        if disk.image() != image || disk.stats().writes != writes {
            return Err("the refused open changed the disk".into());
        }
        // The copy comes back (the same bit, flipped again).
        disk.corrupt(SUPER_NAME, at, 0x01)
            .map_err(|e| e.to_string())?;
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &want, "with the copy back")
    }
    caught(scenario, Mutants::RECLAIM_KEEPS_EPOCH, everywhere);
    caught(scenario, Mutants::NO_HEADER_FUTURE_CHECK, everywhere);
}

/// The one case in which the older superblock copy is taken: nothing
/// was written since the newest one. Then the older copy describes the
/// whole log, with the start it had before the last reclaim: the open
/// succeeds, and holds every record the lost copy's log held and, in
/// front of them, the records that reclaim gave up. Nothing is missing
/// at the end, and a second open finds the same.
#[test]
fn a_lost_superblock_copy_with_nothing_written_since_gives_the_older_start() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut tag = 1;
        let a = fill_to_roll(&mut log, env, &mut tag)?;
        let b = fill_to_roll(&mut log, env, &mut tag)?;
        s(log.sync())?;
        s(log.reclaim(a[a.len() - 1].1.start))?;
        let older: Vec<u8> = tags(&log)?;
        s(log.reclaim(b[b.len() - 1].1.start))?;
        let newest: Vec<u8> = tags(&log)?;
        if newest.len() >= older.len() || !older.ends_with(&newest) {
            return Err("the second reclaim gave nothing up".into());
        }
        drop(log);
        disk.kill();
        disk.corrupt(
            SUPER_NAME,
            current_copy(&disk) * SUPER_COPY_STRIDE + 70,
            0x01,
        )
        .map_err(|e| e.to_string())?;
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &older, "with the newest copy lost")?;
        drop(log);
        disk.power_cut(LoseAll);
        let (log, _) = env.reopen(&disk)?;
        expect(&log, &older, "after a power cut")
    }
    holds(scenario);
}

/// The key comes from the operating system's generator: two logs made
/// one after the other have different keys.
#[test]
fn every_new_log_has_its_own_key() {
    let identity = Env {
        mutants: Mutants::default(),
        align: 64,
        garbage: false,
        seed: 0,
    }
    .identity();
    let a = Log::create(SimDisk::new(0), identity).unwrap();
    let b = Log::create(SimDisk::new(0), identity).unwrap();
    assert!(a.key != b.key);
    assert!(a.key != Key::PUBLIC);
}

/// Which superblock copy is the current one: the valid one with the
/// higher counter.
pub(super) fn current_copy(disk: &SimDisk) -> u64 {
    let file = disk.open(SUPER_NAME).unwrap();
    let counter = |copy: u64| {
        let mut raw = [0u8; SECTOR_LEN];
        file.read_at(copy * SUPER_COPY_STRIDE, &mut raw).unwrap();
        Superblock::decode(&raw).map(|block| block.counter).ok()
    };
    u64::from(counter(1) > counter(0))
}

/// A lost superblock copy. Losing the older copy changes nothing.
/// Losing the current one is an error whenever anything was written
/// since it was: here every recovery writes its seal in the epoch it
/// starts, so the log holds something newer than the older copy, and
/// that copy cannot say where the log starts or what was durable.
/// (After reclaims, and with nothing written since: the two tests
/// above.) Opening with it must not give a log that ends
/// where the newer epoch begins. With both copies lost there is no log.
#[test]
fn a_lost_superblock_copy_is_noticed() {
    fn scenario(env: &Env) -> Outcome {
        let flip = |disk: &SimDisk, copy: u64| {
            disk.corrupt(SUPER_NAME, copy * SUPER_COPY_STRIDE + 70, 0x01)
                .map_err(|e| e.to_string())
        };
        let (disk, mut log) = env.fresh()?;
        put(&mut log, env, 1)?;
        drop(log);
        disk.kill();
        // The older copy.
        flip(&disk, 1 - current_copy(&disk))?;
        let (mut log, _) = env.reopen(&disk)?;
        expect(&log, &[1], "with the older copy lost")?;
        // The current copy. (That open wrote the damaged one afresh.)
        put(&mut log, env, 2)?;
        drop(log);
        disk.kill();
        let current = current_copy(&disk);
        flip(&disk, current)?;
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Corrupt(Corruption::SuperblockBehind { .. })) => {}
            Err(other) => return Err(other.to_string()),
            Ok(_) => return Err("the log opened with a stale superblock".into()),
        }
        flip(&disk, 1 - current)?;
        disk.kill();
        match Log::open(disk.clone(), env.options()) {
            Err(LogError::Corrupt(Corruption::NoSuperblock)) => Ok(()),
            Err(other) => Err(other.to_string()),
            Ok(_) => Err("a log without a superblock opened".into()),
        }
    }
    holds(scenario);
}

/// Failure is announced first (design §4.2): the observer is called
/// once, when the log decides it has failed and before the marker is
/// attempted, whether the marker can then be written or not, and also
/// when the owner fails the log.
#[test]
fn the_observer_is_told_before_the_marker_is_attempted() {
    use std::sync::{Arc, Mutex};
    fn scenario(env: &Env) -> Outcome {
        // The log's own failure, the marker writable and not; and the
        // owner's.
        for (halt, own) in [(false, true), (true, true), (false, false)] {
            let (disk, mut log) = env.fresh()?;
            // What the observer saw: whether the marker was there.
            let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
            let (watch, record) = (disk.clone(), seen.clone());
            log.set_observer(Observer::new(move || {
                let there = watch
                    .list()
                    .is_ok_and(|names| names.iter().any(|n| n == MARKER_NAME));
                record.lock().unwrap().push(there);
            }));
            s(put_raw(&mut log, env, 1))?;
            if !seen.lock().unwrap().is_empty() {
                return Err("the observer was called on a healthy log".into());
            }
            if !own {
                log.mark_failed();
            } else if halt {
                disk.halt_after(0);
                if log.sync().is_ok() {
                    return Err("a sync succeeded on a halted disk".into());
                }
            } else {
                disk.fail_sync(0, false);
                if log.sync().is_ok() {
                    return Err("the failing sync succeeded".into());
                }
            }
            // Nothing more is announced.
            let _ = log.sync();
            log.mark_failed();
            let seen = seen.lock().unwrap().clone();
            if seen != [false] {
                return Err(format!(
                    "the observer saw the marker there: {seen:?} (halted {halt}, own failure {own})"
                ));
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::OBSERVE_AFTER_MARKER, everywhere);
}

/// The cursor a reclaim keeps at the log's start changes what a replay
/// costs and nothing else: after every reclaim, to each record's start
/// in turn and across segments, a replay of what is left gives the
/// records that are left, and so does a new instance, which has no
/// cursor. Mutant: a reclaim that keeps the cursor it had and calls it
/// current; a replay from it does not validate, and says so.
#[test]
fn a_replay_from_the_kept_cursor_gives_the_same_records() {
    fn scenario(env: &Env) -> Outcome {
        let (disk, mut log) = env.fresh()?;
        let mut starts = Vec::new();
        for tag in 1..=40u8 {
            starts.push(put(&mut log, env, tag)?.start);
            if tag % 3 == 0 {
                s(log.sync())?;
            }
        }
        s(log.sync())?;
        if log.segment_files() < 3 {
            return Err("the records fit in fewer than three segments".into());
        }
        for (i, start) in starts.iter().enumerate().skip(1) {
            s(log.reclaim(*start))?;
            let want: Vec<u8> = (i as u8 + 1..=40).collect();
            expect(&log, &want, "after a reclaim, from the kept cursor")?;
            // A part of what is left, beginning behind the start.
            if let Some(later) = starts.get(i + 2) {
                let got = s(tags_between(&log, *later, log.tail()))?;
                if got != want[2..] {
                    return Err(format!("from a later record: {got:?}"));
                }
            }
            if i % 7 == 0 {
                drop(log);
                log = env.reopen(&disk)?.0;
                expect(&log, &want, "in a new instance, without a cursor")?;
            }
        }
        Ok(())
    }
    caught(scenario, Mutants::STALE_CURSOR, everywhere);
}

/// What a checkpoint in batches costs the log is linear in the records:
/// 700 records in one segment, taken in turn by a replay of the next
/// one and a reclaim behind it, validate a few records each time and
/// not the segment's prefix again. Without the kept cursor this is
/// about 490,000 validations (measured); with it, under 3,500.
#[test]
fn replays_from_the_start_do_not_walk_the_segment_again() {
    let env = environments(0)
        .into_iter()
        .find(|env| env.align == 64)
        .unwrap();
    let (_disk, mut log) = env.fresh().unwrap();
    let mut records = Vec::new();
    for n in 0..700u64 {
        records.push(log.append(Kind::Discard, n * 4096, 4096, &[]).unwrap());
    }
    log.sync().unwrap();
    assert_eq!(log.segment_files(), 1, "the records are in one segment");
    let before = log.walked();
    for record in &records {
        let mut seen = 0;
        log.replay(record.start, record.end, &mut |_| seen += 1)
            .unwrap();
        assert_eq!(seen, 1);
        log.reclaim(record.end).unwrap();
    }
    let walked = log.walked() - before;
    assert!(walked < 5 * 700, "{walked} records validated");
}
