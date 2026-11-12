use super::*;
use crate::lifecycle::{Failure, Held, Reason, Shape};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

fn counters(reads: u64, held: u64) -> Counters {
    Counters {
        reads,
        in_flight: held,
        ..Counters::default()
    }
}

/// Times whose fields in the file are 100, 101, and so on, in the
/// file's order, but for the two that say whether the hint is off,
/// which are a yes or a no and so 1 ([`OFF_FIELDS`]).
fn sample_timing() -> skyzfs_local::store::Timing {
    let mut bytes = [0u8; LEN];
    for field in 0..TIME_FIELDS {
        let at = 200 + 8 * field;
        bytes[at..at + 8].copy_from_slice(&sample_field(field).to_le_bytes());
    }
    timing_at(&bytes, false, true)
}

/// The pump's times with the same fields: 242 to 258.
fn sample_pump() -> PumpTiming {
    let mut bytes = [0u8; LEN];
    for field in 0..TIME_FIELDS {
        let at = 200 + 8 * field;
        bytes[at..at + 8].copy_from_slice(&(100 + field as u64).to_le_bytes());
    }
    pump_at(&bytes, false)
}

/// The time fields that say whether the hint is off for the log and
/// for the home image.
const OFF_FIELDS: [usize; 2] = [176, 181];

/// What the sample has in time field `field`.
fn sample_field(field: usize) -> u64 {
    if OFF_FIELDS.contains(&field) {
        1
    } else {
        100 + field as u64
    }
}

fn sample() -> Status {
    Status {
        version: VERSION,
        pid: 0x0102_0304,
        queues: 0x1112,
        depth: 0x2122,
        boot: [7; 16],
        identity: [3; 16],
        written_ms: 1,
        counters: Counters {
            reads: 2,
            writes: 3,
            flushes: 4,
            discards: 5,
            write_zeroes: 6,
            refused: 7,
            in_flight: 8,
            unstored: 9,
        },
        failures: 10,
        first_write_copies: 11,
        recovered_unacknowledged: 12,
        log: Some(LogStatus {
            shown: LogShown {
                live: 13,
                steps: 14,
                stepping: true,
                own_syncs: 18,
                zeroes: ZeroesShown::Written,
                timing: Some(sample_timing()),
                pump: Some(sample_pump()),
            },
            start_ms: 16,
            replayed_bytes: 17,
        }),
    }
}

#[test]
fn the_file_has_the_documented_layout() {
    let bytes = encode(&sample());
    let mut expected = Vec::new();
    expected.extend(b"SKYZSTA7");
    expected.extend([4, 3, 2, 1, 0x12, 0x11, 0x22, 0x21]);
    expected.extend([7; 16]);
    expected.extend([3; 16]);
    // The step under way is 1 at offset 160, between 14 and 16.
    // How write-zeroes are applied is 2, written, at offset 192.
    // The times are the 207 fields from offset 200, here 100 to 306
    // but for the two that say the hint is off, 1.
    for field in (1..=14u64)
        .chain([1, 16, 17, 18, 2])
        .chain((0..TIME_FIELDS).map(sample_field))
    {
        expected.extend(field.to_le_bytes());
    }
    assert_eq!(TIME_FIELDS, 207);
    assert_eq!(bytes.len(), 1856);
    assert_eq!(bytes.to_vec(), expected);
    // Where the times lie, by what they are.
    let timing = sample_timing();
    assert_eq!(timing.requests[0].wait.count, 100);
    assert_eq!(timing.requests[0].call.max_ns, 104);
    assert_eq!(timing.requests[1].wait.sum_ns, 106);
    assert_eq!(timing.requests[4].call.count, 120);
    assert_eq!(timing.step_walk.sum_ns, 125);
    assert_eq!(timing.step_save.sum_ns, 127);
    assert_eq!(timing.step_home_sync.max_ns, 132);
    assert_eq!(timing.step_end.sum_ns, 135);
    assert_eq!(timing.step_own_sync.sum_ns, 137);
    assert_eq!((timing.gate.count, timing.gate.max_ns), (139, 141));
    assert_eq!((timing.worker[0], timing.worker[14]), (142, 156));
    assert_eq!((timing.wait_by[0][0], timing.wait_by[4][14]), (157, 231));
    assert_eq!(timing.wait_by[1][9], 181);
    assert_eq!(timing.longest_wait_cause, [232, 233, 234, 235, 236]);
    assert_eq!(timing.longest_wait_cause_ns, [237, 238, 239, 240, 241]);
    let pump = sample_pump();
    assert_eq!((pump.to_submit.count, pump.to_submit.max_ns), (242, 244));
    assert_eq!(pump.read_to_commit.sum_ns, 246);
    assert_eq!(pump.other_to_commit.count, 248);
    assert_eq!((pump.store.sum_ns, pump.behind_store), (252, 254));
    // The times to the taking have no count in the file: it is the
    // commits'.
    let taken = (pump.read_to_taken, pump.other_to_taken);
    assert_eq!(
        (taken.0.count, taken.0.sum_ns, taken.0.max_ns),
        (245, 255, 256)
    );
    assert_eq!(
        (taken.1.count, taken.1.sum_ns, taken.1.max_ns),
        (248, 257, 258)
    );
    let barriers = (timing.barrier_home, timing.barrier_reuse);
    assert_eq!(
        (barriers.0.count, barriers.0.sum_ns, barriers.0.max_ns),
        (259, 260, 261)
    );
    assert_eq!(barriers.1.max_ns, 264);
    assert_eq!(timing.reader, [265, 266, 267]);
    assert_eq!(timing.read_unflushed, [268, 269, 270]);
    assert_eq!((timing.worker_as_of_ns, timing.reader_as_of_ns), (271, 272));
    let hinted = |count, bytes, ns, refused| skyzfs_local::writeback::Hinted {
        count,
        bytes,
        ns,
        off: true,
        refused,
    };
    assert_eq!(
        timing.writeback,
        [hinted(273, 274, 275, 277), hinted(278, 279, 280, 282)]
    );
    let synced = |at: u64| skyzfs_local::store::Synced {
        count: at,
        sum_ns: at + 1,
        bytes: at + 2,
        handed: at + 3,
    };
    assert_eq!(timing.syncs[0], synced(283));
    assert_eq!(timing.syncs[3], synced(295));
    assert_eq!(timing.syncs[5], synced(303));
    assert_eq!(decode(&bytes), Some(sample()));
}

/// A file of the second version (144 bytes, `SKYZSTA2`) is still read:
/// everything it has, and nothing of the log. The report then has no
/// key of the log.
#[test]
fn a_file_of_the_second_version_is_read_without_the_log() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut second = encode(&sample())[..144].to_vec();
    second[..8].copy_from_slice(b"SKYZSTA2");
    fs::write(&path, &second).expect("write");
    let read = read(&path).expect("read").expect("a status");
    assert_eq!(
        read,
        Status {
            version: 2,
            log: None,
            ..sample()
        }
    );
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "writes"), Some("3"));
    assert_eq!(value(&lines, "log_live"), None);
    // The third version's magic at the second's length is no file.
    second[..8].copy_from_slice(b"SKYZSTA3");
    fs::write(&path, &second).expect("write");
    assert!(super::read(&path).is_err());
}

/// A file of the third version (192 bytes, `SKYZSTA3`) is still read:
/// everything it has, and nothing on how write-zeroes are applied, of
/// which the report then has no line. The fourth version says it, in
/// each of its three states, and a value it does not know is read as
/// not said.
#[test]
fn a_file_of_the_third_version_is_read_and_the_fourth_says_how_zeroes_are_applied() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut third = encode(&sample())[..192].to_vec();
    third[..8].copy_from_slice(b"SKYZSTA3");
    fs::write(&path, &third).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let mut unsaid = Status {
        version: 3,
        ..sample()
    };
    if let Some(log) = unsaid.log.as_mut() {
        log.shown.zeroes = ZeroesShown::Unsaid;
        log.shown.timing = None;
        log.shown.pump = None;
    }
    assert_eq!(read, unsaid);
    let lines_of = |status: &Status| {
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(status),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(&read);
    assert_eq!(value(&lines, "checkpoint_syncs"), Some("18"));
    assert_eq!(value(&lines, "zeroes_applied"), None);
    // The fourth version's magic at the third's length is no file.
    third[..8].copy_from_slice(b"SKYZSTA4");
    fs::write(&path, &third).expect("write");
    assert!(super::read(&path).is_err());

    for (zeroes, says) in [
        (ZeroesShown::Untried, "not yet"),
        (ZeroesShown::Deallocated, "deallocated in the home image"),
        (ZeroesShown::Written, "written as zeros: the file system"),
        (
            ZeroesShown::WrittenForBlocks,
            "written as zeros: the blocks of the volume's file system",
        ),
    ] {
        let mut status = sample();
        if let Some(log) = status.log.as_mut() {
            log.shown.zeroes = zeroes;
        }
        assert_eq!(decode(&encode(&status)), Some(status));
        let lines = lines_of(&status);
        let said = value(&lines, "zeroes_applied").unwrap_or_default();
        assert!(said.starts_with(says), "{zeroes:?}: {said}");
    }
    let mut unknown = encode(&sample());
    unknown[192..200].copy_from_slice(&7u64.to_le_bytes());
    if let Some(log) = unsaid.log.as_mut() {
        log.shown.timing = Some(sample_timing());
        log.shown.pump = Some(sample_pump());
    }
    unsaid.version = VERSION;
    assert_eq!(decode(&unknown), Some(unsaid));
}

/// A file of the fourth version (200 bytes, `SKYZSTA4`) is still read:
/// everything it has, and no times, of which the report then has no
/// line. The fifth version has them, one line each under a key of its
/// own, in the file's order.
#[test]
fn a_file_of_the_fourth_version_is_read_and_the_fifth_says_where_the_time_went() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut fourth = encode(&sample())[..200].to_vec();
    fourth[..8].copy_from_slice(b"SKYZSTA4");
    fs::write(&path, &fourth).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let mut untimed = Status {
        version: 4,
        ..sample()
    };
    if let Some(log) = untimed.log.as_mut() {
        log.shown.timing = None;
        log.shown.pump = None;
    }
    assert_eq!(read, untimed);
    let lines_of = |status: &Status| {
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(status),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(&read);
    assert_eq!(
        value(&lines, "zeroes_applied").map(|s| &s[..7]),
        Some("written")
    );
    assert!(lines.iter().all(|(key, _)| !key.starts_with("time_")));
    // The fifth version's magic at the fourth's length is no file, nor
    // the fourth's at the fifth's.
    fourth[..8].copy_from_slice(b"SKYZSTA5");
    fs::write(&path, &fourth).expect("write");
    assert!(super::read(&path).is_err());
    let mut fifth = encode(&sample()).to_vec();
    fifth[..8].copy_from_slice(b"SKYZSTA4");
    fs::write(&path, &fifth).expect("write");
    assert!(super::read(&path).is_err());
    // Nor the sixth's at the fifth's length, nor the fifth's at the
    // sixth's.
    let mut short = encode(&sample())[..1424].to_vec();
    fs::write(&path, &short).expect("write");
    assert!(super::read(&path).is_err());
    short[..8].copy_from_slice(b"SKYZSTA5");
    let mut long = encode(&sample()).to_vec();
    long[..8].copy_from_slice(b"SKYZSTA5");
    fs::write(&path, &long).expect("write");
    assert!(super::read(&path).is_err());

    let lines = lines_of(&sample());
    let timed: Vec<(&str, &str)> = lines
        .iter()
        .filter(|(key, _)| key.starts_with("time_"))
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    assert_eq!(timed.len(), TIME_FIELDS);
    for (n, (key, value)) in timed.iter().enumerate() {
        assert_eq!(*key, TIME_KEYS[n]);
        // A cause the reader has no name for is printed as its number.
        assert_eq!(*value, sample_field(n).to_string(), "{key}");
    }
    // A cause is said by its name.
    let mut named = sample();
    if let Some(timing) = named.log.as_mut().and_then(|log| log.shown.timing.as_mut()) {
        timing.longest_wait_cause = [9, 3, 0, 13, 14];
    }
    let lines = lines_of(&named);
    for (key, cause) in [
        ("time_read_longest_wait_cause", "step_home_sync"),
        ("time_write_longest_wait_cause", "flush"),
        ("time_flush_longest_wait_cause", "idle"),
        ("time_discard_longest_wait_cause", "other"),
        ("time_zeroes_longest_wait_cause", "step_barrier"),
    ] {
        assert_eq!(value(&lines, key), Some(cause), "{key}");
    }
    for (key, value) in [
        ("time_read_longest_wait_cause_ns", "237"),
        ("time_worker_idle_ns", "142"),
        ("time_worker_step_barrier_ns", "156"),
        ("time_pump_submit_ns", "243"),
        ("time_pump_commit_read_max_ns", "247"),
        ("time_pump_store_count", "251"),
        ("time_pump_store_completions", "254"),
        ("time_write_wait_by_step_home_sync_ns", "181"),
        ("time_zeroes_wait_by_step_barrier_ns", "231"),
        ("time_barrier_home_count", "259"),
        ("time_barrier_reuse_max_ns", "264"),
        ("time_reader_idle_ns", "265"),
        ("time_reader_other_ns", "267"),
        ("time_read_unflushed_count", "268"),
        ("time_read_unflushed_mismatched", "270"),
        ("time_worker_as_of_ns", "271"),
        ("time_reader_as_of_ns", "272"),
        ("time_writeback_log_count", "273"),
        ("time_writeback_log_off", "1"),
        ("time_writeback_log_refused", "277"),
        ("time_writeback_home_bytes", "279"),
        ("time_sync_flush_count", "283"),
        ("time_sync_home_bytes", "297"),
        ("time_sync_superblock_handed", "306"),
    ] {
        assert_eq!(self::value(&lines, key), Some(value), "{key}");
    }
    for (key, value) in [
        ("time_read_count", "100"),
        ("time_read_wait_ns", "101"),
        ("time_write_call_max_ns", "109"),
        ("time_zeroes_count", "120"),
        ("time_step_walk_ns", "125"),
        ("time_step_home_sync_ns", "131"),
        ("time_step_end_max_ns", "136"),
        ("time_step_own_sync_max_ns", "138"),
        ("time_gate_ns", "140"),
        ("time_read_wait_by_step_walk_ns", "163"),
    ] {
        assert_eq!(value_of(&timed, key), Some(value), "{key}");
    }
    let mut keys: Vec<&str> = TIME_KEYS.to_vec();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), TIME_FIELDS);
}

/// A file of the sixth version (1584 bytes, `SKYZSTA6`) is still read:
/// everything where it was, and the hint and the syncs at zero.
#[test]
fn a_file_of_the_sixth_version_is_read_without_the_hint_and_the_syncs() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut sixth = encode(&sample())[..1584].to_vec();
    sixth[..8].copy_from_slice(b"SKYZSTA6");
    fs::write(&path, &sixth).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let timing = read.log.expect("the log").shown.timing.expect("times");
    let mut expected = sample_timing();
    expected.writeback = Default::default();
    expected.syncs = Default::default();
    assert_eq!(timing, expected);
    // The sixth version's magic at the seventh's length is no file,
    // nor the seventh's at the sixth's.
    let mut long = encode(&sample()).to_vec();
    long[..8].copy_from_slice(b"SKYZSTA6");
    fs::write(&path, &long).expect("write");
    assert!(super::read(&path).is_err());
    sixth[..8].copy_from_slice(b"SKYZSTA7");
    fs::write(&path, &sixth).expect("write");
    assert!(super::read(&path).is_err());
}

/// A file of the fifth version (1424 bytes, `SKYZSTA5`) is still read:
/// its fourteen activities where they were, the fifteenth at zero, and
/// nothing from the barriers on. Its fields here are 100, 101, and so
/// on from offset 200, as the fifth version's own layout test had them.
#[test]
fn a_file_of_the_fifth_version_is_read_with_its_fourteen_activities() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut fifth = encode(&sample())[..1424].to_vec();
    fifth[..8].copy_from_slice(b"SKYZSTA5");
    for field in 0..153 {
        let at = 200 + 8 * field;
        fifth[at..at + 8].copy_from_slice(&(100 + field as u64).to_le_bytes());
    }
    fs::write(&path, &fifth).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let log = read.log.expect("the log");
    let (timing, pump) = (
        log.shown.timing.expect("times"),
        log.shown.pump.expect("pump"),
    );
    assert_eq!(
        (timing.worker[0], timing.worker[13], timing.worker[14]),
        (142, 155, 0)
    );
    assert_eq!((timing.wait_by[0][0], timing.wait_by[4][13]), (156, 225));
    assert_eq!((timing.wait_by[0][14], timing.wait_by[4][14]), (0, 0));
    assert_eq!(timing.wait_by[1][9], 179);
    assert_eq!(timing.longest_wait_cause, [226, 227, 228, 229, 230]);
    assert_eq!(timing.longest_wait_cause_ns, [231, 232, 233, 234, 235]);
    assert_eq!((pump.to_submit.count, pump.to_submit.max_ns), (236, 238));
    assert_eq!((pump.store.sum_ns, pump.behind_store), (246, 248));
    assert_eq!(
        (pump.read_to_taken.sum_ns, pump.other_to_taken.max_ns),
        (249, 252)
    );
    assert_eq!(timing.barrier_home, Spent::default());
    assert_eq!(timing.barrier_reuse, Spent::default());
    assert_eq!((timing.reader, timing.read_unflushed), ([0; 3], [0; 3]));
}

fn value_of<'a>(lines: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    lines.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The report shows the log: its live size, the steps taken, whether
/// one is under way, and what the start took and replayed.
#[test]
fn the_report_shows_the_log_and_the_start() {
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&sample()),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "log_live"), Some("13"));
    assert_eq!(value(&lines, "checkpoint_steps"), Some("14"));
    assert_eq!(value(&lines, "checkpoint_syncs"), Some("18"));
    assert_eq!(value(&lines, "checkpoint_step"), Some("under way"));
    assert_eq!(value(&lines, "start_ms"), Some("16"));
    assert_eq!(value(&lines, "replayed_bytes"), Some("17"));
    let resting = Status {
        log: Some(LogStatus::default()),
        ..sample()
    };
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&resting),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "checkpoint_step"), Some("none"));
}

/// A change of the log that is posted "at once" is written without
/// waiting for the interval, each one: the begin and the end of a step
/// are both in the file's history although they are closer together
/// than the interval. A change posted without it keeps the pace.
#[test]
fn a_steps_begin_and_end_are_written_at_once() {
    let board = Arc::new(Board::new());
    let (wrote, written) = mpsc::channel::<LogShown>();
    let publisher = std::thread::spawn({
        let board = board.clone();
        // An interval no test waits for.
        move || {
            board.publish_with_log(Pace::new(3_600_000), |_, log, _| {
                wrote.send(*log).map_err(|_| io::Error::other("gone"))
            })
        }
    });
    let wait = Duration::from_secs(10);
    // The first write: nothing was written yet.
    assert_eq!(written.recv_timeout(wait), Ok(LogShown::default()));
    let begun = LogShown {
        live: 100,
        steps: 0,
        stepping: true,
        own_syncs: 0,
        ..LogShown::default()
    };
    board.post_log(begun, true);
    assert_eq!(written.recv_timeout(wait), Ok(begun));
    let ended = LogShown {
        live: 60,
        steps: 1,
        stepping: false,
        own_syncs: 1,
        ..LogShown::default()
    };
    board.post_log(ended, true);
    assert_eq!(written.recv_timeout(wait), Ok(ended));
    // Not at once: it waits for the interval, which never ends here.
    let grown = LogShown { live: 90, ..ended };
    board.post_log(grown, false);
    assert!(written.recv_timeout(Duration::from_millis(100)).is_err());
    // The stop writes what is posted.
    board.stop();
    assert_eq!(written.recv_timeout(wait), Ok(grown));
    assert_eq!(publisher.join().expect("the publisher"), 0);
}

/// A file of the first version (128 bytes, `SKYZSTA1`) is no status,
/// and not an error: nothing that wrote one ever served.
#[test]
fn a_file_of_the_first_version_reads_as_no_status() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut old = encode(&sample())[..128].to_vec();
    old[..8].copy_from_slice(b"SKYZSTA1");
    fs::write(&path, &old).expect("write");
    assert_eq!(read(&path).expect("read"), None);
    // The new magic at the old length is a part of a file: an error.
    old[..8].copy_from_slice(b"SKYZSTA2");
    fs::write(&path, &old).expect("write");
    assert!(read(&path).is_err());
}

#[test]
fn the_file_is_replaced_whole_and_a_part_of_one_is_an_error() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    assert_eq!(read(&path).expect("read"), None);
    write(&path, &sample()).expect("write");
    assert_eq!(read(&path).expect("read"), Some(sample()));

    // The old file, held open, keeps its bytes: the new one is another
    // file under the same name.
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let inode = fs::metadata(&path).expect("there").ino();
    let mut held = fs::File::open(&path).expect("open");
    let mut next = sample();
    next.counters.reads = 99;
    write(&path, &next).expect("write");
    assert_ne!(fs::metadata(&path).expect("there").ino(), inode);
    let mut old = Vec::new();
    held.read_to_end(&mut old).expect("read");
    assert_eq!(decode(&old), Some(sample()));
    assert_eq!(read(&path).expect("read"), Some(next));
    assert_eq!(fs::read_dir(dir.path()).expect("list").count(), 1);

    let whole = encode(&sample());
    for len in 0..whole.len() {
        fs::write(&path, &whole[..len]).expect("write");
        assert!(read(&path).is_err(), "{len} bytes were read as a status");
    }
}

#[test]
fn the_uptime_is_read_in_milliseconds() {
    assert_eq!(parse_uptime("12345.67 54321.00\n"), Some(12_345_670));
    assert_eq!(parse_uptime("0.05 1.00"), Some(50));
    assert_eq!(parse_uptime("7 1"), Some(7000));
    assert_eq!(parse_uptime("7.1234"), Some(7123));
    for wrong in ["", "x", "1.x", "-1.0", "+1.0", ".5"] {
        assert_eq!(parse_uptime(wrong), None, "{wrong:?}");
    }
    if cfg!(target_os = "linux") {
        let (a, b) = (uptime_ms().expect("uptime"), uptime_ms().expect("uptime"));
        assert!(a > 0 && b >= a);
    }
}

/// Drive a pace with a clock of our own. `changes` are (time, reads);
/// between them and up to `end` the pace is asked every millisecond,
/// as the publishing thread would be woken. Returns the writes as
/// (time, reads).
fn drive(mutants: u32, changes: &[(u64, u64)], end: u64, fails_at: &[u64]) -> Vec<(u64, u64)> {
    let mut pace = Pace::new(100).with(mutants);
    let mut current = Counters::default();
    let mut writes = Vec::new();
    // The thread sleeps until a change or until the time it was told.
    let mut wake_at: Option<u64> = Some(0);
    for now in 0..=end {
        let changed = changes.iter().find(|(at, _)| *at == now);
        if let Some((_, reads)) = changed {
            current = counters(*reads, 0);
        }
        if changed.is_none() && wake_at != Some(now) {
            continue;
        }
        wake_at = None;
        match pace.due(&current, now) {
            Due::Never => {}
            Due::At(at) => wake_at = Some(at),
            Due::Now if fails_at.contains(&now) => {
                pace.failed(now);
                writes.push((now, u64::MAX));
                wake_at = Some(now + 1);
            }
            Due::Now => {
                pace.wrote(current, now);
                writes.push((now, current.reads));
                wake_at = Some(now + 1);
            }
        }
    }
    writes
}

fn converges(mutants: u32) -> Result<(), String> {
    // A burst: a change every 10 ms for 250 ms, then nothing.
    let changes: Vec<(u64, u64)> = (1..=25).map(|i| (i * 10, i)).collect();
    let writes = drive(mutants, &changes, 2000, &[]);
    let last = writes.last().ok_or("nothing was written")?;
    if last.1 != 25 {
        return Err(format!("the last change was never written: {writes:?}"));
    }
    if last.0 > 250 + 100 {
        return Err(format!("the last change was written at {}", last.0));
    }
    Ok(())
}

fn paced(mutants: u32) -> Result<(), String> {
    let changes: Vec<(u64, u64)> = (1..=25).map(|i| (i * 10, i)).collect();
    let writes = drive(mutants, &changes, 2000, &[]);
    for pair in writes.windows(2) {
        if pair[1].0 - pair[0].0 < 100 {
            return Err(format!("two writes within the interval: {pair:?}"));
        }
    }
    Ok(())
}

fn quiet(mutants: u32) -> Result<(), String> {
    let writes = drive(mutants, &[(10, 1)], 2000, &[]);
    // The first counters (all zero) at the start, and the one change.
    if writes != [(0, 0), (100, 1)] {
        return Err(format!("{writes:?}"));
    }
    Ok(())
}

fn a_failure_waits(mutants: u32) -> Result<(), String> {
    let writes = drive(mutants, &[(150, 1)], 2000, &[150, 151, 152]);
    if writes != [(0, 0), (150, u64::MAX), (250, 1)] {
        return Err(format!("{writes:?}"));
    }
    Ok(())
}

#[test]
fn the_pace_converges_is_quiet_and_keeps_its_interval() {
    for (name, rule, defect) in [
        (
            "converges",
            converges as fn(u32) -> Result<(), String>,
            Mutants::NEEDS_ANOTHER_CHANGE,
        ),
        ("paced", paced, Mutants::NO_INTERVAL),
        ("quiet", quiet, Mutants::REWRITES_UNCHANGED),
        ("a failure waits", a_failure_waits, Mutants::RETRIES_AT_ONCE),
    ] {
        assert_eq!(rule(0), Ok(()), "{name}");
        assert!(
            rule(defect).is_err(),
            "the defect against `{name}` is not noticed"
        );
    }
}

/// The publishing thread, for real: a burst, then an idle queue.
#[test]
fn after_a_burst_the_file_shows_the_last_change_with_no_further_request() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let board = Arc::new(Board::new());
    let written = Arc::new(AtomicU64::new(0));
    let thread = {
        let (board, path, written) = (board.clone(), path.clone(), written.clone());
        std::thread::spawn(move || {
            board.publish(Pace::new(50), |counters, failures| {
                written.fetch_add(1, Ordering::SeqCst);
                let mut status = sample();
                status.counters = *counters;
                status.failures = failures;
                write(&path, &status)
            })
        })
    };
    for i in 1..=200 {
        board.post(counters(i, 200 - i));
        if i % 20 == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // The queue is idle now: held is 0 and nothing more is posted. The
    // bound is one interval and one write; the test allows a loaded
    // machine far more, and fails only if the change never comes.
    let shown = |path: &Path| read(path).ok().flatten().map(|s| s.counters);
    let start = Instant::now();
    while shown(&path) != Some(counters(200, 0)) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the last change was not written: {:?}",
            shown(&path)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // And then nothing is rewritten while nothing changes.
    let writes = written.load(Ordering::SeqCst);
    assert!(writes < 200, "paced: {writes} writes for 200 changes");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(written.load(Ordering::SeqCst), writes);
    // Posting the same counters wakes nothing.
    board.post(counters(200, 0));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(written.load(Ordering::SeqCst), writes);

    board.stop();
    assert_eq!(thread.join().expect("the thread"), 0);
}

#[test]
fn a_write_that_blocks_or_fails_holds_no_post_and_the_last_counters_are_written_at_the_stop() {
    let board = Arc::new(Board::new());
    let (entered_tx, entered) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let (seen_tx, seen) = mpsc::channel();
    let thread = {
        let board = board.clone();
        std::thread::spawn(move || {
            let mut first = true;
            board.publish(Pace::new(10_000), move |counters, failures| {
                if first {
                    // The first write hangs, and then fails.
                    first = false;
                    let _ = entered_tx.send(());
                    let _ = released.recv();
                    return Err(io::Error::other("no space"));
                }
                let _ = seen_tx.send((*counters, failures));
                Ok(())
            })
        })
    };
    // The publishing thread is inside its write. Posts return at once.
    entered.recv().expect("the write began");
    let start = Instant::now();
    for i in 1..=1000 {
        board.post(counters(i, 0));
    }
    assert!(start.elapsed() < Duration::from_secs(5));
    // The stop writes the last counters though the interval (ten
    // seconds) is far from over, and the failure is counted.
    board.stop();
    release.send(()).expect("release");
    assert_eq!(thread.join().expect("the thread"), 1);
    assert_eq!(
        seen.try_iter().collect::<Vec<_>>(),
        [(counters(1000, 0), 1)]
    );
}

fn located(state: DeviceState, whose: Whose) -> Located {
    Located {
        number: 4,
        state,
        shape: Shape {
            size: 1 << 30,
            queues: 2,
            depth: 32,
            flags: 0,
        },
        whose,
    }
}

fn value<'a>(lines: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    lines
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn the_report_has_the_keys_the_rig_reads_and_says_whose_the_counters_are() {
    let status = sample();
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        251,
    );
    let keys: Vec<&str> = lines.iter().map(|(k, _)| *k).collect();
    // The times are between how write-zeroes are applied and the pid,
    // in the order of `TIME_KEYS`; the other keys are as they were.
    let at = keys.iter().position(|k| *k == "zeroes_applied").unwrap() + 1;
    assert_eq!(keys[at..at + TIME_FIELDS], TIME_KEYS);
    let keys: Vec<&str> = keys
        .into_iter()
        .filter(|k| !k.starts_with("time_"))
        .collect();
    assert_eq!(
        keys,
        [
            "state",
            "device",
            "queues",
            "depth",
            "reads",
            "writes",
            "flushes",
            "discards",
            "write_zeroes",
            "refused",
            "held",
            "unstored",
            "first_write_copies",
            "recovered_unacknowledged",
            "status_write_failures",
            "age_ms",
            "status_version",
            "log_live",
            "checkpoint_steps",
            "checkpoint_syncs",
            "checkpoint_step",
            "start_ms",
            "replayed_bytes",
            "zeroes_applied",
            "pid",
            "counters"
        ]
    );
    // The process to signal: only while the kernel shows a live server.
    assert_eq!(value(&lines, "pid"), Some("16909060"));
    assert_eq!(value(&lines, "state"), Some("serving"));
    assert_eq!(value(&lines, "device"), Some("/dev/ublkb4"));
    // The device's own values, not the file's.
    assert_eq!(value(&lines, "queues"), Some("2"));
    assert_eq!(value(&lines, "depth"), Some("32"));
    assert_eq!(value(&lines, "reads"), Some("2"));
    assert_eq!(value(&lines, "held"), Some("8"));
    assert_eq!(value(&lines, "unstored"), Some("9"));
    assert_eq!(value(&lines, "age_ms"), Some("250"));
    assert_eq!(
        value(&lines, "counters"),
        Some("of the serving daemon, pid 16909060")
    );

    // No live server: the counters were left by a daemon that is gone.
    for (state, said) in [
        (DeviceState::Waiting, "waiting for a server"),
        (
            DeviceState::Dead,
            "dead: never started, or deleted and still held",
        ),
    ] {
        let lines = report(
            Some(&located(state, Whose::Ours)),
            Some(&status),
            Noted::Nothing,
            &[7; 16],
            251,
        );
        assert_eq!(value(&lines, "state"), Some(said));
        assert_eq!(value(&lines, "pid"), None);
        assert_eq!(
            value(&lines, "counters"),
            Some("left by a daemon that is gone, pid 16909060")
        );
    }
}

#[test]
fn the_report_shows_no_device_that_is_not_this_volumes_and_no_counters_of_another_boot() {
    let status = sample();
    let lines = report(None, None, Noted::Nothing, &[7; 16], 0);
    assert_eq!(lines, [("state", "detached".to_string())]);

    // Detached, with the file a daemon left: its counters, marked.
    let lines = report(None, Some(&status), Noted::Nothing, &[7; 16], 251);
    assert_eq!(value(&lines, "state"), Some("detached"));
    assert_eq!(value(&lines, "device"), None);
    assert_eq!(value(&lines, "queues"), Some("4370"));
    assert!(value(&lines, "counters").is_some_and(|c| c.starts_with("left by")));

    // A file of another boot says nothing about this one.
    let lines = report(None, Some(&status), Noted::Nothing, &[8; 16], 251);
    assert_eq!(lines, [("state", "detached".to_string())]);

    for (whose, begins) in [
        (Whose::Anothers, "detached: device 4"),
        (Whose::Unsure, "unknown: device 4"),
    ] {
        let lines = report(
            Some(&located(DeviceState::Live, whose)),
            Some(&status),
            Noted::Nothing,
            &[7; 16],
            251,
        );
        assert!(value(&lines, "state").is_some_and(|s| s.starts_with(begins)));
        assert_eq!(value(&lines, "device"), None, "not shown as this volume's");
        assert!(value(&lines, "counters").is_some_and(|c| c.starts_with("left by")));
    }
}

#[test]
fn the_report_shows_a_failure_of_this_boot_and_what_it_needs() {
    let stored = |held: bool| StoredFailure {
        boot: [7; 16],
        failure: Failure {
            held: held.into(),
            reason: Reason::LogFailed,
            identity: [3; 16],
        },
        written_ms: 1000,
    };
    let dead = located(DeviceState::Dead, Whose::Ours);
    for (found, noted, state) in [
        (
            Some(&dead),
            Noted::Whole(stored(true)),
            "failed, held by a client: not served again in this boot",
        ),
        (
            None,
            Noted::Whole(stored(false)),
            "failed: needs --acknowledge-failure",
        ),
        (
            None,
            Noted::Unreadable,
            "failed, held by a client: not served again in this boot",
        ),
    ] {
        let lines = report(found, None, noted, &[7; 16], 1250);
        assert_eq!(value(&lines, "state"), Some(state));
        assert_eq!(value(&lines, "device").is_some(), found.is_some());
        if noted == Noted::Unreadable {
            assert_eq!(
                value(&lines, "failure"),
                Some("the failure note cannot be read")
            );
            assert_eq!(value(&lines, "failure_held"), None);
            continue;
        }
        let held = matches!(noted, Noted::Whole(s) if s.failure.held == Held::Yes);
        assert_eq!(
            value(&lines, "failure"),
            Some("the log failed while the volume was served")
        );
        assert_eq!(
            value(&lines, "failure_held"),
            Some(if held { "yes" } else { "no" })
        );
        assert_eq!(
            value(&lines, "failure_boot"),
            Some("07070707070707070707070707070707")
        );
        assert_eq!(value(&lines, "failure_age_ms"), Some("250"));
    }

    // A note of another boot is not shown and decides nothing.
    let lines = report(None, None, Noted::Whole(stored(true)), &[8; 16], 1250);
    assert_eq!(value(&lines, "state"), Some("detached"));
    assert_eq!(value(&lines, "failure"), None);
    assert_eq!(value(&lines, "failure_device"), None);
    assert_eq!(value(&lines, "failure_pool"), None);

    // With a failure of this boot, and no daemon and no status file:
    // what became of the device, in each state it can be found in, and
    // what to do about a pool on it.
    let waiting = located(DeviceState::Waiting, Whose::Ours);
    let unsure = located(DeviceState::Waiting, Whose::Unsure);
    // What a failure under an imported pool leaves: the words are the
    // refused start's.
    let unsure_dead = located(DeviceState::Dead, Whose::Unsure);
    let anothers = located(DeviceState::Live, Whose::Anothers);
    for (found, says) in [
        (None, "none under the volume's recorded number"),
        (Some(&waiting), "waits for a server it will not get"),
        (Some(&dead), "deleted, and something still holds it"),
        (
            Some(&unsure),
            "no note, and was left: deleting it by hand, or a reboot",
        ),
        (
            Some(&unsure_dead),
            "already deleted and still held open by a client",
        ),
        (Some(&anothers), "is not this volume's"),
    ] {
        for noted in [Noted::Whole(stored(false)), Noted::Unreadable] {
            let lines = report(found, None, noted, &[7; 16], 1250);
            let device = value(&lines, "failure_device").unwrap_or_default();
            assert!(device.contains(says), "{found:?}: {device}");
            assert_eq!(value(&lines, "failure_pool"), Some(POOL_WARNING));
            assert_eq!(
                lines.last().map(|(key, _)| *key),
                Some("failure_pool"),
                "the last line"
            );
        }
    }
    // The step it names is a read, and it does not advise an export,
    // which is not known to work in that state.
    assert!(POOL_WARNING.contains("zpool scrub") && POOL_WARNING.contains("reboot"));
    assert!(!POOL_WARNING.contains("export"));
}

/// The order of a start: the publishing thread's first write is tried
/// before whoever waits for it goes on (the daemon then says `ready`),
/// also when that write fails; and nobody is told it was tried before
/// it was.
#[test]
fn the_first_write_is_tried_before_the_waiter_goes_on_also_when_it_fails() {
    for fails in [false, true] {
        let board = Arc::new(Board::new());
        assert!(
            !board.first_write_tried(Duration::from_millis(20)),
            "nothing publishes yet"
        );
        let tried = Arc::new(AtomicU64::new(0));
        let gate = Arc::new(std::sync::Barrier::new(2));
        let thread = {
            let (board, tried, gate) = (board.clone(), tried.clone(), gate.clone());
            std::thread::spawn(move || {
                board.publish(Pace::new(50), |_, _| {
                    if tried.load(Ordering::SeqCst) == 0 {
                        // The first write is held until the test lets
                        // it go.
                        gate.wait();
                    }
                    tried.fetch_add(1, Ordering::SeqCst);
                    if fails {
                        Err(std::io::Error::other("no"))
                    } else {
                        Ok(())
                    }
                })
            })
        };
        // The write is under way and not finished: not tried yet.
        assert!(!board.first_write_tried(Duration::from_millis(50)));
        assert_eq!(tried.load(Ordering::SeqCst), 0);
        gate.wait();
        assert!(board.first_write_tried(Duration::from_secs(20)));
        assert!(tried.load(Ordering::SeqCst) >= 1, "fails: {fails}");
        board.stop();
        let failures = thread.join().expect("the publishing thread");
        assert_eq!(failures > 0, fails);
    }
}

/// Waiting for the first write posts nothing: counters the queue's
/// thread has put up before the start is over are the ones that are
/// written, then and at the end, and are not replaced by zeros.
#[test]
fn waiting_for_the_first_write_leaves_the_posted_counters_as_they_are() {
    let board = Arc::new(Board::new());
    board.post(counters(7, 2));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let thread = {
        let (board, seen) = (board.clone(), seen.clone());
        std::thread::spawn(move || {
            board.publish(Pace::new(10), |counters, _| {
                seen.lock().expect("lock").push(*counters);
                Ok(())
            })
        })
    };
    assert!(board.first_write_tried(Duration::from_secs(20)));
    // No request follows: the queue's thread posts nothing more.
    std::thread::sleep(Duration::from_millis(100));
    board.stop();
    thread.join().expect("the publishing thread");
    let seen = seen.lock().expect("lock");
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|written| *written == counters(7, 2)),
        "{seen:?}"
    );
}

/// `write_again` has the file written once more with what is posted,
/// at once and whatever the interval, and replaces nothing: for a
/// value the writer reads elsewhere (the start's time) that changed
/// after the first write.
#[test]
fn write_again_writes_what_is_posted_once_more_and_replaces_nothing() {
    let board = Arc::new(Board::new());
    board.post(counters(7, 2));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let thread = {
        let (board, seen) = (board.clone(), seen.clone());
        std::thread::spawn(move || {
            // An interval far beyond the test: only `write_again`
            // explains a second write.
            board.publish(Pace::new(3_600_000), |counters, _| {
                seen.lock().expect("lock").push(*counters);
                Ok(())
            })
        })
    };
    assert!(board.first_write_tried(Duration::from_secs(20)));
    assert_eq!(seen.lock().expect("lock").len(), 1);
    board.write_again();
    let began = Instant::now();
    while seen.lock().expect("lock").len() < 2 {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "not written again"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    board.stop();
    thread.join().expect("the publishing thread");
    let seen = seen.lock().expect("lock");
    assert!(
        seen.iter().all(|written| *written == counters(7, 2)),
        "{seen:?}"
    );
}

/// A board that publishes with `pace` and a write that fails at its
/// second attempt only; the times of the attempts go to the list.
fn fails_second(
    pace: Pace,
) -> (
    Arc<Board>,
    Arc<Mutex<Vec<Instant>>>,
    std::thread::JoinHandle<u64>,
) {
    let board = Arc::new(Board::new());
    board.post(counters(7, 2));
    let tried = Arc::new(Mutex::new(Vec::new()));
    let thread = {
        let (board, tried) = (board.clone(), tried.clone());
        std::thread::spawn(move || {
            board.publish(pace, |_, _| {
                let mut tried = tried.lock().expect("lock");
                tried.push(Instant::now());
                if tried.len() == 2 {
                    Err(io::Error::other("the second write fails"))
                } else {
                    Ok(())
                }
            })
        })
    };
    assert!(board.first_write_tried(Duration::from_secs(20)));
    (board, tried, thread)
}

/// A write asked for with `write_again` is owed until one succeeds. On
/// an idle board (nothing posted changes) the write that fails is
/// tried again, an interval later and not at once. Before, nothing
/// remembered it: the counters and the log equalled what was written
/// last, so the value the write was asked for (the start's time)
/// stayed out of the file for as long as the volume was idle. Mutants:
/// the owed write is forgotten; a failed write is tried again at once.
#[test]
fn a_write_asked_for_again_that_fails_is_tried_again_an_interval_later() {
    const INTERVAL: Duration = Duration::from_millis(400);
    fn scenario(mutants: u32) -> Result<(), String> {
        let pace = Pace::new(INTERVAL.as_millis() as u64).with(mutants);
        let (board, tried, thread) = fails_second(pace);
        // Beyond the interval of the first write, so that what holds
        // the retry back is the failed write's own interval.
        std::thread::sleep(INTERVAL + Duration::from_millis(50));
        board.write_again();
        let began = Instant::now();
        let third = loop {
            if let Some(third) = tried.lock().expect("lock").get(2).copied() {
                break Some(third);
            }
            if began.elapsed() > Duration::from_secs(4) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let second = tried.lock().expect("lock").get(1).copied();
        board.stop();
        let failures = thread.join().expect("the publishing thread");
        let (Some(second), Some(third)) = (second, third) else {
            return Err("the write that failed was not tried again in 4 s".into());
        };
        // The pace counts the interval from when the failed write
        // returned, which is after the time noted inside it.
        if third.duration_since(second) < INTERVAL {
            return Err(format!(
                "tried again after {:?}, sooner than the interval",
                third.duration_since(second)
            ));
        }
        if failures != 1 {
            return Err(format!("{failures} failures"));
        }
        Ok(())
    }
    assert_eq!(scenario(0), Ok(()));
    assert!(scenario(Mutants::OWED_FORGOTTEN).is_err(), "forgotten");
    assert!(scenario(Mutants::RETRIES_AT_ONCE).is_err(), "at once");
}

/// A write that is still owed at the stop is written by the stop, also
/// when the interval has not passed. Mutant: the owed write is
/// forgotten, and the stop writes nothing.
#[test]
fn a_write_still_owed_at_the_stop_is_written_by_the_stop() {
    fn scenario(mutants: u32) -> Result<(), String> {
        let (board, tried, thread) = fails_second(Pace::new(3_600_000).with(mutants));
        board.write_again();
        let began = Instant::now();
        while tried.lock().expect("lock").len() < 2 {
            if began.elapsed() > Duration::from_secs(20) {
                return Err("not written again".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        board.stop();
        let failures = thread.join().expect("the publishing thread");
        let attempts = tried.lock().expect("lock").len();
        if (attempts, failures) != (3, 1) {
            return Err(format!("{attempts} attempts, {failures} failures"));
        }
        Ok(())
    }
    assert_eq!(scenario(0), Ok(()));
    assert!(scenario(Mutants::OWED_FORGOTTEN).is_err(), "forgotten");
}

/// `status` says which version of the file it read (`status_version`),
/// not a `time_` key: a script that reads the times tells by it the
/// zeros of fields an older file did not have from counted zeros.
#[test]
fn status_prints_the_version_of_the_file_it_read() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    for (len, magic, version) in [
        (1424, b"SKYZSTA5", "5"),
        (1584, b"SKYZSTA6", "6"),
        (1856, b"SKYZSTA7", "7"),
    ] {
        let mut bytes = encode(&sample())[..len].to_vec();
        bytes[..8].copy_from_slice(magic);
        fs::write(&path, &bytes).expect("write");
        let read = read(&path).expect("read").expect("a status");
        let lines = report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        );
        assert_eq!(value(&lines, "status_version"), Some(version));
        assert_eq!(read.version.to_string(), version);
    }
    assert_eq!(VERSION, 7);
}
