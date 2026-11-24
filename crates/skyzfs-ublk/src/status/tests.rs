use super::*;
use crate::lifecycle::{Failure, Held, Reason, Shape};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

fn counters(reads: u64, held: u64) -> Counters {
    Counters {
        reads,
        in_flight: held,
        ..Counters::default()
    }
}

/// The pump's times whose fields in the file are 100 to 108, in the
/// file's order.
fn sample_pump() -> PumpTiming {
    pump_of(&std::array::from_fn(|i| 100 + i as u64))
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
            commands_aborted: 26,
            commands_failed: 27,
            last_command_failure: -28,
        },
        failures: 10,
        first_write_copies: 11,
        start_ms: 16,
        shown: StoreShown {
            zeroes: ZeroesShown::WrittenDirect,
            pump: sample_pump(),
            io_queue: Some(IoQueue {
                threads: 20,
                in_flight: 21,
                waiting: 22,
                waited: 23,
                in_flight_max: 24,
                waiting_max: 25,
            }),
            cuts: Some(Cuts {
                count: 29,
                newest: Some(Cut {
                    number: 29,
                    group: 30,
                    itself: true,
                }),
            }),
            held: Some(HeldShown {
                state: CutStateShown::TakenUp,
                held: 2,
                copied: 31,
                bound: 32,
                abandoned: 33,
                replaced: Some(46),
                holes: Some(HolesShown {
                    state: CutHolesShown::Held,
                    chunks: 63,
                }),
            }),
            intent: Some(IntentShown {
                state: CutStateShown::TakenUp,
                region_bytes: 34,
                regions: 35,
                named: 36,
                named_at_start: 37,
                syncs: Spent {
                    count: 38,
                    sum_ns: 39,
                    max_ns: 40,
                },
                waits: 41,
                header: Some(HeaderSyncShown {
                    on: true,
                    syncs: Spent {
                        count: 47,
                        sum_ns: 48,
                        max_ns: 49,
                    },
                }),
            }),
            upload: Some(sample_upload()),
            channel: Some(ChannelShown {
                state: ChannelStateShown::NoUpload,
                answered: 70,
                peer: 71,
                malformed: 72,
            }),
        },
    }
}

/// The uploader, with every field set apart: committing cut 42.
fn sample_upload() -> UploadShown {
    UploadShown {
        state: UploadStateShown::Committing,
        cut: 42,
        wait_ms: 43,
        lease: LeaseShown::Held,
        epoch: 44,
        expires_unix_ms: 45,
        figures: UploadFigures {
            zero_blocks: Some(62),
            not_offered: Some(64),
            lost_whole: Some(78),
            ..UploadFigures::of(std::array::from_fn(|i| 50 + i as u64))
        },
        checked: CheckedShown::NotTheBase,
        rule: RuleShown::Whole,
        why: Text::new("the why"),
        holder: Text::new("node-a"),
        last_error: Text::new("object storage: gone"),
        publish: Some(sample_publish()),
    }
}

/// The publishes, with every field set apart.
fn sample_publish() -> PublishShown {
    PublishShown {
        accepted: 65,
        published: 66,
        found: 67,
        refused: 68,
        expired: 69,
        withdrawn: 73,
        pending: 74,
        oldest_ms: 75,
        last_cut: 76,
        last_group: 77,
        oldest: Text::new("snap-old"),
        last: Text::new("snap-last"),
    }
}

/// The file is the table at the top of `status.rs`, field by field, and
/// is read back as it was written.
#[test]
fn the_file_has_the_documented_layout() {
    let bytes = encode(&sample());
    assert_eq!(bytes.len(), 1416);
    assert_eq!(&bytes[..8], b"SKYZST19");
    assert_eq!(bytes[8..12], 0x0102_0304u32.to_le_bytes());
    assert_eq!(bytes[12..14], 0x1112u16.to_le_bytes());
    assert_eq!(bytes[14..16], 0x2122u16.to_le_bytes());
    assert_eq!(bytes[16..32], [7; 16]);
    assert_eq!(bytes[32..48], [3; 16]);
    let mut fields = vec![
        (48, 1),
        (56, 2),
        (64, 3),
        (72, 4),
        (80, 5),
        (88, 6),
        (96, 7),
        (104, 8),
        (112, 10),
        (120, 11),
        (128, 16),
        (136, 4),
    ];
    fields.extend((0..9).map(|i| (144 + 8 * i, 100 + i as u64)));
    fields.extend([
        (216, 20),
        (224, 21),
        (232, 22),
        (240, 23),
        (248, 24),
        (256, 25),
        (264, 26),
        (272, 27),
        (288, 29),
        (296, 30),
        (304, 1),
        (312, 1),
        (320, 2),
        (328, 31),
        (336, 32),
        (344, 33),
        (352, 1),
        (360, 34),
        (368, 35),
        (376, 36),
        (384, 37),
        (392, 38),
        (400, 39),
        (408, 40),
        (416, 41),
        (424, 7),
        (432, 42),
        (440, 43),
        (448, 1),
        (456, 44),
        (464, 45),
    ]);
    fields.extend((0..12).map(|i| (472 + 8 * i, 50 + i as u64)));
    fields.extend([
        (568, 3),
        (576, 2),
        (968, 46),
        (976, 1),
        (984, 47),
        (992, 48),
        (1000, 49),
        (1008, 1),
        (1016, 63),
        (1024, 62),
        (1032, 64),
        (1040, 65),
        (1048, 66),
        (1056, 67),
        (1064, 68),
        (1072, 69),
        (1080, 73),
        (1088, 74),
        (1096, 75),
        (1104, 76),
        (1112, 77),
        (1376, 2),
        (1384, 70),
        (1392, 71),
        (1400, 72),
        (1408, 78),
    ]);
    for (at, value) in fields {
        assert_eq!(u64_at(&bytes, at), value, "the field at {at}");
    }
    assert_eq!(bytes[280..284], (-28i32).to_le_bytes());
    assert_eq!(bytes[284..288], [0; 4]);
    for (at, text) in [
        (584, "the why"),
        (712, "node-a"),
        (840, "object storage: gone"),
        (1120, "snap-old"),
        (1248, "snap-last"),
    ] {
        assert_eq!(
            &bytes[at..at + text.len()],
            text.as_bytes(),
            "the text at {at}"
        );
        assert!(
            bytes[at + text.len()..at + 128].iter().all(|b| *b == 0),
            "padded at {at}"
        );
    }
    assert_eq!(decode(&bytes), Some(sample()));
}

/// Before the first cut the file holds a count of 0 and a group of 0,
/// which read back as no newest cut.
#[test]
fn before_the_first_cut_the_file_has_no_group() {
    let mut status = sample();
    status.shown.cuts = Some(Cuts::default());
    let bytes = encode(&status);
    assert_eq!(u64_at(&bytes, 288), 0);
    assert_eq!(u64_at(&bytes, 296), 0);
    assert_eq!(u64_at(&bytes, 304), 0);
    assert_eq!(decode(&bytes), Some(status));
}

/// A file of the tenth version (2032 bytes, `SKYZST10`) as a daemon
/// with the in-place store wrote it: the fields this version keeps at
/// the tenth's places, and every other byte, the log store's fields
/// among them, 0x5a, so that a field read from a wrong place shows.
fn tenth_file() -> Vec<u8> {
    let status = sample();
    let mut bytes = vec![0x5au8; 2032];
    bytes[..8].copy_from_slice(b"SKYZST10");
    bytes[8..48].copy_from_slice(&encode(&status)[8..48]);
    let mut put = |at: usize, value: u64| bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    let c = &status.counters;
    for (i, value) in [
        status.written_ms,
        c.reads,
        c.writes,
        c.flushes,
        c.discards,
        c.write_zeroes,
        c.refused,
        c.in_flight,
    ]
    .into_iter()
    .enumerate()
    {
        put(48 + 8 * i, value);
    }
    put(tenth::FAILURES, status.failures);
    put(tenth::FIRST_WRITE_COPIES, status.first_write_copies);
    put(tenth::START_MS, status.start_ms);
    put(tenth::ZEROES, 4);
    // The time to the taking, at 1440, stays 0x5a: it is not read.
    let pump = pump_fields(&status.shown.pump);
    for (i, value) in pump.into_iter().enumerate() {
        put(tenth::PUMP + 8 * i, value);
    }
    let q = status.shown.io_queue.unwrap_or_default();
    for (i, value) in [
        q.threads,
        q.in_flight,
        q.waiting,
        q.waited,
        q.in_flight_max,
        q.waiting_max,
    ]
    .into_iter()
    .enumerate()
    {
        put(tenth::IO_QUEUE + 8 * i, value);
    }
    put(tenth::COMMANDS, c.commands_aborted);
    put(tenth::COMMANDS + 8, c.commands_failed);
    put(
        tenth::COMMANDS + 16,
        u64::from(u32::from_le_bytes(c.last_command_failure.to_le_bytes())),
    );
    bytes
}

/// The sample as a file of an older version holds it: `version`, no
/// cuts, and nothing of what that version did not have.
fn as_of(version: u32) -> Status {
    let mut status = sample();
    status.version = version;
    status.shown.cuts = None;
    status.shown.held = None;
    status.shown.intent = None;
    status.shown.upload = None;
    status.shown.channel = None;
    if version < 10 {
        status.counters.commands_aborted = 0;
        status.counters.commands_failed = 0;
        status.counters.last_command_failure = 0;
    }
    if version < 9 {
        status.shown.io_queue = None;
    }
    status
}

/// The keys only the log store's versions printed. `status` prints none
/// of them, whatever version it read (M2.1; testing's M2.T0).
const LOG_STORE_KEYS: [&str; 7] = [
    "unstored",
    "recovered_unacknowledged",
    "log_live",
    "checkpoint_steps",
    "checkpoint_syncs",
    "checkpoint_step",
    "replayed_bytes",
];

/// Whether the report has a key of the log store's versions.
fn has_log_store_keys(lines: &[(&'static str, String)]) -> bool {
    lines
        .iter()
        .any(|(key, _)| LOG_STORE_KEYS.contains(key) || key.starts_with("time_"))
}

/// Files of the tenth, ninth (2008 bytes, `SKYZSTA9`) and eighth (1960
/// bytes, `SKYZSTA8`) versions are still read: what this version keeps,
/// from the tenth's places; the commands the driver ended otherwise
/// than OK only from the tenth, the I/O threads' queue from the ninth
/// on; no cuts, and none of the log store's fields. The report shows no
/// key that the file did not hold.
#[test]
fn files_of_the_eighth_to_the_tenth_version_are_read_without_the_log_store() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    for (version, len, magic) in [
        (10, 2032, b"SKYZST10"),
        (9, 2008, b"SKYZSTA9"),
        (8, 1960, b"SKYZSTA8"),
    ] {
        let mut bytes = tenth_file()[..len].to_vec();
        bytes[..8].copy_from_slice(magic);
        fs::write(&path, &bytes).expect("write");
        let read = read(&path).expect("read").expect("a status");
        assert_eq!(read, as_of(version), "version {version}");
        let lines = report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        );
        assert!(!has_log_store_keys(&lines), "version {version}: {lines:?}");
        assert_eq!(value(&lines, "cuts"), None);
        assert_eq!(value(&lines, "cut_group"), None);
        assert_eq!(
            value(&lines, "status_version"),
            Some(version.to_string().as_str())
        );
        assert_eq!(value(&lines, "start_ms"), Some("16"));
        assert_eq!(value(&lines, "first_write_copies"), Some("11"));
        assert_eq!(value(&lines, "status_write_failures"), Some("10"));
        assert_eq!(value(&lines, "pump_submit_count"), Some("100"));
        assert_eq!(value(&lines, "pump_commit_other_max_ns"), Some("108"));
        assert!(
            !lines.iter().any(|(key, _)| key.starts_with("pump_taken_")),
            "version {version}: {lines:?}"
        );
        assert_eq!(
            lines.iter().any(|(key, _)| key.starts_with("commands_")),
            version >= 10
        );
        assert_eq!(value(&lines, "io_threads"), (version >= 9).then_some("20"));
    }
}

/// A magic at another version's length is no file, in each direction.
#[test]
fn a_magic_at_another_versions_length_is_an_error() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let versions: [(&[u8; 8], usize); 12] = [
        (b"SKYZST19", 1416),
        (b"SKYZST18", 1408),
        (b"SKYZST17", 1032),
        (b"SKYZST16", 1008),
        (b"SKYZST15", 976),
        (b"SKYZST14", 424),
        (b"SKYZST13", 352),
        (b"SKYZST12", 312),
        (b"SKYZST11", 336),
        (b"SKYZST10", 2032),
        (b"SKYZSTA9", 2008),
        (b"SKYZSTA8", 1960),
    ];
    for (magic, _) in versions {
        for (other, len) in versions {
            if magic == other {
                continue;
            }
            let mut bytes = vec![0u8; len];
            bytes[..8].copy_from_slice(magic);
            fs::write(&path, &bytes).expect("write");
            assert!(
                read(&path).is_err(),
                "{} at {len} bytes",
                String::from_utf8_lossy(magic)
            );
        }
    }
}

/// A file of a version before the eighth (from a daemon before M1.3m)
/// is no status, and not an error. Its magic at another length is.
#[test]
fn a_file_of_a_version_before_the_eighth_reads_as_no_status() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    for (len, magic) in [
        (1856, b"SKYZSTA7"),
        (1584, b"SKYZSTA6"),
        (1424, b"SKYZSTA5"),
        (200, b"SKYZSTA4"),
        (192, b"SKYZSTA3"),
        (144, b"SKYZSTA2"),
        (128, b"SKYZSTA1"),
    ] {
        let mut bytes = vec![0x5au8; len];
        bytes[..8].copy_from_slice(magic);
        fs::write(&path, &bytes).expect("write");
        assert_eq!(read(&path).expect("read"), None, "{len} bytes");
        bytes.push(0);
        fs::write(&path, &bytes).expect("write");
        assert!(read(&path).is_err(), "{} bytes", len + 1);
    }
}

/// The report shows the count of cuts from a file that holds it, and
/// the newest cut's group once there is a cut.
#[test]
fn the_report_shows_the_cuts_and_the_newest_group_once_there_is_one() {
    let mut status = sample();
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cuts"), Some("29"));
    assert_eq!(value(&lines, "cut_group"), Some("30"));
    assert_eq!(value(&lines, "cut_opens"), Some("itself"));
    if let Some(Cuts {
        newest: Some(cut), ..
    }) = status.shown.cuts.as_mut()
    {
        cut.itself = false;
    }
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cut_opens"), Some("rewind"));
    status.shown.cuts = Some(Cuts::default());
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cuts"), Some("0"));
    assert_eq!(value(&lines, "cut_group"), None);
    assert_eq!(value(&lines, "cut_opens"), None);
}

/// The sample as a file of the eleventh version (336 bytes,
/// `SKYZST11`): this version's fields up to the end of the pump's times
/// at the same places, then the four fields of the time to the taking
/// (0x5a, so that a field read from a wrong place shows), then the
/// rest 32 bytes later than here, without whether the newest cut opens
/// as itself.
fn eleventh_file() -> Vec<u8> {
    let twelfth = encode(&sample());
    let mut bytes = vec![0x5au8; 336];
    bytes[..216].copy_from_slice(&twelfth[..216]);
    bytes[248..336].copy_from_slice(&twelfth[216..304]);
    bytes[..8].copy_from_slice(b"SKYZST11");
    bytes
}

/// A file of the thirteenth version (352 bytes, `SKYZST13`): this
/// version's fields up to the intent map, at the same places.
fn thirteenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..352].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST13");
    bytes
}

/// A file of the thirteenth version is still read, with the held cuts
/// and without the intent map, whose keys `status` does not print for
/// it.
#[test]
fn a_file_of_the_thirteenth_version_is_read_without_the_intent_map() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    fs::write(&path, thirteenth_file()).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let mut expected = sample();
    expected.version = 13;
    // The cuts replaced came with the fifteenth version, the image's
    // holes with the seventeenth.
    without_holes(&mut expected);
    if let Some(held) = &mut expected.shown.held {
        held.replaced = None;
    }
    expected.shown.upload = None;
    expected.shown.intent = None;
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cuts_held"), Some("2"));
    assert!(!lines.iter().any(|(key, _)| key.starts_with("intent_")));
}

/// A file of the fourteenth version (424 bytes, `SKYZST14`): this
/// version's fields up to the uploader, at the same places.
fn fourteenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..424].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST14");
    bytes
}

/// A file of the fourteenth version is still read, with the intent map
/// and without the uploader, whose keys `status` does not print for it.
#[test]
fn a_file_of_the_fourteenth_version_is_read_without_the_uploader() {
    let read = decode(&fourteenth_file()).expect("a status");
    let mut expected = sample();
    expected.version = 14;
    // The cuts replaced came with the fifteenth version, the header's
    // sync with the sixteenth, the image's holes with the seventeenth.
    without_holes(&mut expected);
    if let Some(held) = &mut expected.shown.held {
        held.replaced = None;
    }
    if let Some(intent) = &mut expected.shown.intent {
        intent.header = None;
    }
    expected.shown.upload = None;
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "intent_named"), Some("36"));
    assert!(!lines
        .iter()
        .any(|(key, _)| key.starts_with("upload") || key.starts_with("lease")));
}

/// What a file before the nineteenth version does not say: the whole
/// cuts lost (M2.4f).
fn without_whole_lost(status: &mut Status) {
    if let Some(upload) = &mut status.shown.upload {
        upload.figures.lost_whole = None;
    }
}

/// What a file before the eighteenth version does not say: the cuts not
/// offered, the publishes and the channel (M2.5-2), and what a file
/// before the nineteenth does not say.
fn without_publishes(status: &mut Status) {
    without_whole_lost(status);
    if let Some(upload) = &mut status.shown.upload {
        upload.figures.not_offered = None;
        upload.publish = None;
    }
    status.shown.channel = None;
}

/// A file of the eighteenth version (1408 bytes, `SKYZST18`): this
/// version's fields up to the whole cuts lost, at the same places.
fn eighteenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..1408].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST18");
    bytes
}

/// A file of the eighteenth version is still read, with the publishes
/// and the channel and without the whole cuts lost (M2.4f), whose key
/// `status` does not print for it.
#[test]
fn a_file_of_the_eighteenth_version_is_read_without_the_whole_cuts_lost() {
    let read = decode(&eighteenth_file()).expect("a status");
    let mut expected = sample();
    expected.version = 18;
    without_whole_lost(&mut expected);
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "upload_cuts_lost"), Some("59"));
    assert_eq!(value(&lines, "publish_accepted"), Some("65"));
    assert_eq!(value(&lines, "control_malformed"), Some("72"));
    assert_eq!(value(&lines, "upload_cuts_lost_whole"), None);
}

/// The whole cuts lost are printed from a nineteenth file, after the
/// cuts not offered, zero where none was; and the uploader that waits
/// after one says so, with the wait (M2.4f).
#[test]
fn status_prints_the_whole_cuts_lost_and_the_wait_after_one() {
    let lines_of = |status: &Status| {
        let read = decode(&encode(status)).expect("a status");
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(&sample());
    assert_eq!(value(&lines, "upload_cuts_lost_whole"), Some("78"));
    let at = |key: &str| lines.iter().position(|(k, _)| *k == key).expect(key);
    assert!(at("upload_not_offered") < at("upload_cuts_lost_whole"));
    assert!(at("upload_cuts_lost_whole") < at("publish_accepted"));

    let mut waiting = sample();
    let mut upload = UploadShown {
        state: UploadStateShown::WholeLost,
        wait_ms: 120_000,
        why: Text::new("whole cut 3 went before its commit: abandoned at the bound"),
        ..UploadShown::default()
    };
    upload.figures.lost_whole = Some(0);
    waiting.shown.upload = Some(upload);
    let lines = lines_of(&waiting);
    assert_eq!(value(&lines, "upload_state"), Some("whole_lost"));
    assert_eq!(value(&lines, "upload_wait_ms"), Some("120000"));
    assert_eq!(value(&lines, "upload_cuts_lost_whole"), Some("0"));
    assert_eq!(
        value(&lines, "upload"),
        Some(
            "whole cut 3 went before its commit: abandoned at the bound: no whole cut can be \
             uploaded now, next whole cut after a wait of 120 s"
        )
    );
    assert_eq!(UploadStateShown::of(10), UploadStateShown::WholeLost);
    assert_eq!(UploadStateShown::WholeLost.code(), 10);
    assert_eq!(UploadStateShown::of(11), UploadStateShown::Unsaid);
}

/// A file of the seventeenth version (1032 bytes, `SKYZST17`): this
/// version's fields up to the publishes, at the same places.
fn seventeenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..1032].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST17");
    bytes
}

/// A file of the seventeenth version is still read, with the image's
/// holes and without the cuts not offered, the publishes or the channel
/// (M2.5-2), whose keys `status` does not print for it.
#[test]
fn a_file_of_the_seventeenth_version_is_read_without_the_publishes() {
    let read = decode(&seventeenth_file()).expect("a status");
    let mut expected = sample();
    expected.version = 17;
    without_publishes(&mut expected);
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "upload_zero_blocks"), Some("62"));
    assert_eq!(value(&lines, "cut_hole_chunks"), Some("63"));
    assert!(!lines.iter().any(|(key, _)| key.starts_with("publish")
        || key.starts_with("control")
        || *key == "upload_not_offered"));
}

/// The publishes and the channel are printed from an eighteenth file:
/// every key, whether or not the daemon uploads or has a channel; a
/// name not set is printed as `none`, and a state this version does
/// not know as unsaid.
#[test]
fn status_prints_the_publishes_and_the_channel() {
    let lines_of = |status: &Status| {
        let read = decode(&encode(status)).expect("a status");
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(&sample());
    for (key, value_) in [
        ("upload_not_offered", "64"),
        ("publish_accepted", "65"),
        ("publish_published", "66"),
        ("publish_found", "67"),
        ("publish_refused", "68"),
        ("publish_expired", "69"),
        ("publish_withdrawn", "73"),
        ("publish_pending", "74"),
        ("publish_oldest", "snap-old"),
        ("publish_oldest_ms", "75"),
        ("publish_last", "snap-last"),
        ("publish_last_cut", "76"),
        ("publish_last_group", "77"),
        ("control", "no_upload"),
        ("control_answered", "70"),
        ("control_peer", "71"),
        ("control_malformed", "72"),
    ] {
        assert_eq!(value(&lines, key), Some(value_), "{key}");
    }
    // The publishes after the uploader's other keys, the channel after
    // them.
    let at = |key: &str| lines.iter().position(|(k, _)| *k == key).expect(key);
    assert!(at("upload_last_error") < at("publish_accepted"));
    assert!(at("publish_last_group") < at("control"));
    // A daemon that does not upload and has no channel: zeros, every key.
    let mut quiet = sample();
    quiet.shown.upload = Some(UploadShown::default());
    quiet.shown.channel = None;
    let lines = lines_of(&quiet);
    for (key, value_) in [
        ("upload_not_offered", "0"),
        ("publish_pending", "0"),
        ("publish_oldest", "none"),
        ("publish_last", "none"),
        ("control", "none"),
        ("control_answered", "0"),
    ] {
        assert_eq!(value(&lines, key), Some(value_), "{key}");
    }
    for (state, said) in [
        (ChannelStateShown::Uploads, "uploads"),
        (ChannelStateShown::NoUpload, "no_upload"),
        (ChannelStateShown::Failed, "failed"),
    ] {
        let mut status = sample();
        status.shown.channel = Some(ChannelShown {
            state,
            ..ChannelShown::default()
        });
        assert_eq!(value(&lines_of(&status), "control"), Some(said));
    }
    let mut bytes = encode(&sample());
    bytes[1376..1384].copy_from_slice(&9u64.to_le_bytes());
    let read = decode(&bytes).expect("a status");
    assert_eq!(
        read.shown.channel.map(|c| c.state),
        Some(ChannelStateShown::Unsaid)
    );
}

/// The channel's post is kept by the store's and by the uploader's, and
/// a change of it alone is written.
#[test]
fn the_channels_post_is_kept_and_written_on_its_own() {
    let board = Board::new();
    let channel = ChannelShown {
        state: ChannelStateShown::Uploads,
        answered: 1,
        peer: 0,
        malformed: 0,
    };
    board.post_channel(channel);
    board.post_store(StoreShown::default());
    board.post_upload(sample_upload());
    assert_eq!(board.lock().store.channel, Some(channel));
    let (written, wrote) = mpsc::channel();
    let board = Arc::new(board);
    let publisher = std::thread::spawn({
        let board = board.clone();
        move || {
            board.publish_with_store(Pace::new(1), |_, shown, _| {
                let _ = written.send(shown.channel);
                Ok(())
            })
        }
    });
    let wait = Duration::from_secs(20);
    assert_eq!(wrote.recv_timeout(wait), Ok(Some(channel)));
    let more = ChannelShown {
        answered: 2,
        ..channel
    };
    board.post_channel(more);
    assert_eq!(wrote.recv_timeout(wait), Ok(Some(more)));
    board.stop();
    publisher.join().expect("the publisher");
}

/// What a file before the seventeenth version does not say: the image's
/// holes and the blocks uploaded as zeros (M2.4e).
fn without_holes(status: &mut Status) {
    without_publishes(status);
    if let Some(held) = &mut status.shown.held {
        held.holes = None;
    }
    if let Some(upload) = &mut status.shown.upload {
        upload.figures.zero_blocks = None;
    }
}

/// A file of the sixteenth version (1008 bytes, `SKYZST16`): this
/// version's fields up to the image's holes, at the same places.
fn sixteenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..1008].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST16");
    bytes
}

/// A file of the sixteenth version is still read, with the header's
/// sync and without the image's holes or the blocks uploaded as zeros
/// (M2.4e), whose keys `status` does not print for it.
#[test]
fn a_file_of_the_sixteenth_version_is_read_without_the_holes() {
    let read = decode(&sixteenth_file()).expect("a status");
    let mut expected = sample();
    expected.version = 16;
    without_holes(&mut expected);
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "intent_header_sync_max_ns"), Some("49"));
    assert_eq!(value(&lines, "upload_errors"), Some("61"));
    for key in ["cut_holes", "cut_hole_chunks", "upload_zero_blocks"] {
        assert_eq!(value(&lines, key), None, "{key}");
    }
}

/// A file of the fifteenth version (976 bytes, `SKYZST15`): this
/// version's fields up to the header's sync, at the same places.
fn fifteenth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..976].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST15");
    bytes
}

/// A file of the fifteenth version is still read, with the uploader and
/// the cuts replaced and without the header's sync (M2.4d), whose keys
/// `status` does not print for it.
#[test]
fn a_file_of_the_fifteenth_version_is_read_without_the_header_sync() {
    let read = decode(&fifteenth_file()).expect("a status");
    let mut expected = sample();
    expected.version = 15;
    if let Some(intent) = &mut expected.shown.intent {
        intent.header = None;
    }
    without_holes(&mut expected);
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cuts_replaced"), Some("46"));
    assert_eq!(value(&lines, "intent_waits"), Some("41"));
    assert!(!lines
        .iter()
        .any(|(key, _)| key.starts_with("intent_header")));
}

/// The flag at 976 reads on only where it is 1: any other value is off,
/// as the table says (S04 of #120's review).
#[test]
fn the_header_sync_flag_is_on_only_at_one() {
    for (word, on) in [(0u64, false), (1, true), (2, false), (u64::MAX, false)] {
        let mut bytes = encode(&sample()).to_vec();
        bytes[976..984].copy_from_slice(&word.to_le_bytes());
        let read = decode(&bytes).expect("a status");
        let header = read
            .shown
            .intent
            .and_then(|intent| intent.header)
            .expect("the header's sync");
        assert_eq!(header.on, on, "{word}");
    }
}

/// The uploader is printed from a fifteenth file, every key whether or
/// not the daemon uploads, with its state in words as the lead's
/// decisions on M2.4c give them; an empty text is printed as `none`, and
/// a code this version does not know as unsaid.
#[test]
fn status_prints_the_uploader() {
    let lines_of = |upload: UploadShown| {
        let mut status = sample();
        status.shown.upload = Some(upload);
        let read = decode(&encode(&status)).expect("a status");
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(sample_upload());
    for (key, said) in [
        ("upload", "committing cut 42"),
        ("upload_state", "committing"),
        ("upload_cut", "42"),
        ("upload_wait_ms", "43"),
        ("upload_why", "the why"),
        ("lease", "held"),
        ("lease_holder", "node-a"),
        ("lease_epoch", "44"),
        ("lease_expires_unix_ms", "45"),
        ("uploaded", "50"),
        ("uploaded_whole", "51"),
        ("upload_blocks", "52"),
        ("upload_found_committed", "53"),
        ("upload_committed_gone", "54"),
        ("upload_committed_named", "55"),
        ("upload_committed_kept", "56"),
        ("upload_mismatches", "57"),
        ("upload_abandoned_at_mismatch", "58"),
        ("upload_cuts_lost", "59"),
        ("upload_given_up", "60"),
        ("upload_errors", "61"),
        ("upload_zero_blocks", "62"),
        ("upload_checked", "not_the_base"),
        ("upload_rule", "whole"),
        ("upload_last_error", "object storage: gone"),
    ] {
        assert_eq!(value(&lines, key), Some(said), "{key}");
    }
    let off = lines_of(UploadShown::default());
    assert_eq!(
        value(&off, "upload"),
        Some("off: this daemon does not upload")
    );
    assert_eq!(value(&off, "upload_state"), Some("off"));
    assert_eq!(value(&off, "lease"), Some("none"));
    assert_eq!(value(&off, "lease_holder"), Some("none"));
    assert_eq!(value(&off, "upload_last_error"), Some("none"));
    assert_eq!(value(&off, "uploaded"), Some("0"));
    for (upload, said) in [
        (
            UploadShown {
                state: UploadStateShown::NoLease,
                why: Text::new("held by node-b at epoch 3"),
                ..UploadShown::default()
            },
            "no lease: held by node-b at epoch 3",
        ),
        (
            UploadShown {
                state: UploadStateShown::LeaseLost,
                lease: LeaseShown::Lost,
                holder: Text::new("node-b"),
                epoch: 4,
                ..UploadShown::default()
            },
            "lease lost to node-b at epoch 4",
        ),
        (
            UploadShown {
                state: UploadStateShown::LeaseLost,
                lease: LeaseShown::Lost,
                epoch: 5,
                ..UploadShown::default()
            },
            "lease lost at epoch 5",
        ),
        (
            UploadShown {
                state: UploadStateShown::Backoff,
                why: Text::new("commit of cut 3 refused: conflict"),
                wait_ms: 4000,
                ..UploadShown::default()
            },
            "commit of cut 3 refused: conflict, retry in 4 s",
        ),
        (
            UploadShown {
                state: UploadStateShown::Checking,
                ..UploadShown::default()
            },
            "checking",
        ),
        (
            UploadShown {
                state: UploadStateShown::Stopped,
                ..UploadShown::default()
            },
            "stopped: the store kept a committed cut, and nothing more is uploaded by this \
             instance",
        ),
    ] {
        assert_eq!(value(&lines_of(upload), "upload"), Some(said));
    }
    let mut bytes = encode(&sample());
    for at in [424, 448, 568, 576] {
        bytes[at..at + 8].copy_from_slice(&99u64.to_le_bytes());
    }
    let read = decode(&bytes)
        .expect("a status")
        .shown
        .upload
        .expect("upload");
    assert_eq!(read.state, UploadStateShown::Unsaid);
    assert_eq!(read.lease, LeaseShown::Unsaid);
    assert_eq!(read.checked, CheckedShown::Unsaid);
    assert_eq!(read.rule, RuleShown::Unsaid);
    assert_eq!(read.said(), "unsaid");
}

/// A text is cut to 128 bytes at a character's boundary, and a line's
/// end or any other control character in it is a space, so that
/// `status` prints it on its line.
#[test]
fn a_text_is_cut_at_a_character_and_kept_on_one_line() {
    // 127 bytes, then a character of two: it does not fit whole.
    let long = format!("{}é", "a".repeat(127));
    let text = Text::new(&long);
    assert_eq!(text.said(), "a".repeat(127));
    let exact = "b".repeat(128);
    assert_eq!(Text::new(&exact).said(), exact);
    assert_eq!(
        Text::new("one\ntwo\tthree\0four").said(),
        "one two three four"
    );
    assert_eq!(Text::new("").said(), "");
    assert_eq!(Text::of(&[0xff; 128]).said(), "\u{fffd}".repeat(128));
}

/// The uploader's thread posts its state apart from the queue's: a post
/// of the store keeps it, and a change of it alone is written.
/// Each check's finding keeps its code in the file and its word in
/// `status`, `row_v1` (code 6, F1's N3) among them, with no new version:
/// a reader that does not know a code reads it as not said.
#[test]
fn each_finding_of_the_check_keeps_its_code_and_its_word() {
    for (checked, code, word) in [
        (CheckedShown::Unchecked, 0u64, "unchecked"),
        (CheckedShown::Matched, 1, "matched"),
        (CheckedShown::NoRow, 2, "no_row"),
        (CheckedShown::NotTheBase, 3, "not_the_base"),
        (CheckedShown::HeadMoved, 4, "head_moved"),
        (CheckedShown::Unreadable, 5, "unreadable"),
        (CheckedShown::RowV1, 6, "row_v1"),
    ] {
        let mut status = sample();
        status.shown.upload = Some(UploadShown {
            checked,
            ..sample_upload()
        });
        let bytes = encode(&status);
        assert_eq!(&bytes[568..576], &code.to_le_bytes(), "{word}");
        let read = decode(&bytes).expect("a status");
        let upload = read.shown.upload.as_ref().expect("upload");
        assert_eq!(upload.checked, checked, "{word}");
        assert_eq!(upload.checked.said(), word);
        let lines = report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        );
        assert_eq!(value(&lines, "upload_checked"), Some(word));
    }
}

#[test]
fn the_uploaders_post_is_kept_by_the_stores_and_written_on_its_own() {
    let board = Board::new();
    let upload = sample_upload();
    board.post_upload(upload);
    board.post_store(StoreShown::default());
    assert_eq!(board.lock().store.upload, Some(upload));
    let (written, wrote) = mpsc::channel();
    let board = Arc::new(board);
    let publisher = std::thread::spawn({
        let board = board.clone();
        move || {
            board.publish_with_store(Pace::new(1), |_, shown, _| {
                let _ = written.send(shown.upload);
                Ok(())
            })
        }
    });
    let wait = Duration::from_secs(20);
    assert_eq!(wrote.recv_timeout(wait), Ok(Some(upload)));
    let idle = UploadShown {
        state: UploadStateShown::Idle,
        ..upload
    };
    board.post_upload(idle);
    assert_eq!(wrote.recv_timeout(wait), Ok(Some(idle)));
    board.stop();
    publisher.join().expect("the publisher");
}

/// The intent map is printed from a fourteenth file, every key whether
/// or not the store keeps one; a state this version does not know is
/// printed as unsaid.
#[test]
fn status_prints_the_intent_map() {
    let lines_of = |intent: IntentShown| {
        let mut status = sample();
        status.shown.intent = Some(intent);
        let read = decode(&encode(&status)).expect("a status");
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(sample().shown.intent.expect("intent"));
    for (key, said) in [
        ("intent_state", "taken_up"),
        ("intent_region_bytes", "34"),
        ("intent_regions", "35"),
        ("intent_named", "36"),
        ("intent_named_at_start", "37"),
        ("intent_syncs", "38"),
        ("intent_sync_ns", "39"),
        ("intent_sync_max_ns", "40"),
        ("intent_waits", "41"),
        ("intent_header_sync", "on"),
        ("intent_header_syncs", "47"),
        ("intent_header_sync_ns", "48"),
        ("intent_header_sync_max_ns", "49"),
    ] {
        assert_eq!(value(&lines, key), Some(said), "{key}");
    }
    let off = lines_of(IntentShown::default());
    assert_eq!(value(&off, "intent_state"), Some("off"));
    assert_eq!(value(&off, "intent_named"), Some("0"));
    assert_eq!(value(&off, "intent_header_sync"), Some("off"));
    assert_eq!(value(&off, "intent_header_syncs"), Some("0"));
    let begun = lines_of(IntentShown {
        state: CutStateShown::Begun,
        ..IntentShown::default()
    });
    assert_eq!(value(&begun, "intent_state"), Some("begun"));
    let mut bytes = encode(&sample());
    bytes[352..360].copy_from_slice(&7u64.to_le_bytes());
    let read = decode(&bytes).expect("a status");
    assert_eq!(
        read.shown.intent.map(|intent| intent.state),
        Some(CutStateShown::Unsaid)
    );
}

/// A file of the twelfth version (312 bytes, `SKYZST12`): this
/// version's fields up to the held cuts, at the same places.
fn twelfth_file() -> Vec<u8> {
    let mut bytes = encode(&sample())[..312].to_vec();
    bytes[..8].copy_from_slice(b"SKYZST12");
    bytes
}

/// A file of the twelfth version is still read, without the held cuts,
/// whose keys `status` does not print for it.
#[test]
fn a_file_of_the_twelfth_version_is_read_without_held_cuts() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    fs::write(&path, twelfth_file()).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let mut expected = sample();
    expected.version = 12;
    expected.shown.upload = None;
    expected.shown.channel = None;
    expected.shown.held = None;
    expected.shown.intent = None;
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cut_opens"), Some("itself"));
    assert!(!lines.iter().any(|(key, _)| key.starts_with("cuts_held")
        || key.starts_with("cut_state")
        || key.starts_with("cut_copied")));
}

/// The held cuts are printed from a thirteenth file, every key whether
/// or not the store holds cuts; a state this version does not know is
/// printed as unsaid.
#[test]
fn status_prints_the_held_cuts() {
    let lines_of = |held: HeldShown| {
        let mut status = sample();
        status.shown.held = Some(held);
        let read = decode(&encode(&status)).expect("a status");
        report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&read),
            Noted::Nothing,
            &[7; 16],
            5,
        )
    };
    let lines = lines_of(sample().shown.held.expect("held"));
    for (key, said) in [
        ("cut_state", "taken_up"),
        ("cuts_held", "2"),
        ("cut_copied_bytes", "31"),
        ("cut_bound_bytes", "32"),
        ("cuts_abandoned", "33"),
        ("cuts_replaced", "46"),
        ("cut_holes", "held"),
        ("cut_hole_chunks", "63"),
    ] {
        assert_eq!(value(&lines, key), Some(said), "{key}");
    }
    // A daemon that holds no cut says 0 replaced; a file before the
    // fifteenth version does not say it, and the key is not printed.
    let off = lines_of(HeldShown {
        replaced: Some(0),
        ..HeldShown::default()
    });
    assert_eq!(value(&off, "cut_state"), Some("off"));
    assert_eq!(value(&off, "cuts_held"), Some("0"));
    assert_eq!(value(&off, "cuts_replaced"), Some("0"));
    // A daemon that holds no cut, or whose state has no hole set, says
    // so with no chunk (M2.4e).
    assert_eq!(value(&off, "cut_holes"), Some("none"));
    assert_eq!(value(&off, "cut_hole_chunks"), Some("0"));
    let dropped = lines_of(HeldShown {
        holes: Some(HolesShown {
            state: CutHolesShown::Dropped,
            chunks: 9,
        }),
        ..HeldShown::default()
    });
    assert_eq!(value(&dropped, "cut_holes"), Some("dropped"));
    assert_eq!(value(&dropped, "cut_hole_chunks"), Some("9"));
    let older = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&decode(&fourteenth_file()).expect("a status")),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&older, "cuts_abandoned"), Some("33"));
    assert_eq!(value(&older, "cuts_replaced"), None);
    assert_eq!(value(&older, "cut_holes"), None);
    let begun = lines_of(HeldShown {
        state: CutStateShown::Begun,
        ..HeldShown::default()
    });
    assert_eq!(value(&begun, "cut_state"), Some("begun"));
    let mut bytes = encode(&sample());
    bytes[312..320].copy_from_slice(&7u64.to_le_bytes());
    let read = decode(&bytes).expect("a status");
    assert_eq!(
        read.shown.held.map(|held| held.state),
        Some(CutStateShown::Unsaid)
    );
    let mut bytes = encode(&sample());
    bytes[1008..1016].copy_from_slice(&3u64.to_le_bytes());
    let read = decode(&bytes).expect("a status");
    assert_eq!(
        read.shown.held.and_then(|held| held.holes).map(|h| h.state),
        Some(CutHolesShown::Unsaid)
    );
}

/// A file of the eleventh version is still read: everything from its
/// own places, with the count and the newest cut's group, nothing of
/// the time to the taking, and nothing of whether the newest cut opens
/// as itself, which `status` does not print for it.
#[test]
fn a_file_of_the_eleventh_version_is_read_without_whether_the_cut_opens_as_itself() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    fs::write(&path, eleventh_file()).expect("write");
    let read = read(&path).expect("read").expect("a status");
    let mut expected = sample();
    expected.version = 11;
    expected.shown.upload = None;
    expected.shown.channel = None;
    expected.shown.held = None;
    expected.shown.intent = None;
    if let Some(Cuts {
        newest: Some(cut), ..
    }) = expected.shown.cuts.as_mut()
    {
        cut.itself = false;
    }
    assert_eq!(read, expected);
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&read),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "cuts"), Some("29"));
    assert_eq!(value(&lines, "cut_group"), Some("30"));
    assert_eq!(value(&lines, "cut_opens"), None);
    assert_eq!(value(&lines, "io_threads"), Some("20"));
    assert_eq!(value(&lines, "commands_failed"), Some("27"));
    assert!(!lines.iter().any(|(key, _)| key.starts_with("pump_taken_")));
    assert!(!has_log_store_keys(&lines));
}

/// How write-zeroes are applied is shown only where the file says it: a
/// value this version does not know (3 was the log store's) shows no
/// key.
#[test]
fn the_report_shows_how_write_zeroes_are_applied_only_where_the_file_says() {
    let mut status = sample();
    for (code, shown) in [
        (0, true),
        (1, true),
        (2, true),
        (3, false),
        (4, true),
        (5, false),
    ] {
        status.shown.zeroes = ZeroesShown::of(code);
        let bytes = encode(&status);
        assert_eq!(u64_at(&bytes, 136), if shown { code } else { u64::MAX });
        let lines = report(
            Some(&located(DeviceState::Live, Whose::Ours)),
            Some(&status),
            Noted::Nothing,
            &[7; 16],
            5,
        );
        assert_eq!(value(&lines, "zeroes_applied").is_some(), shown, "{code}");
    }
}

/// The result of the last failed command is shown only once one has
/// failed; the two counts are shown whenever the file holds them.
#[test]
fn the_report_shows_the_last_command_failure_only_after_one() {
    let mut status = sample();
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "commands_aborted"), Some("26"));
    assert_eq!(value(&lines, "commands_failed"), Some("27"));
    assert_eq!(value(&lines, "last_command_failure"), Some("-28"));
    status.counters.commands_failed = 0;
    status.counters.last_command_failure = 0;
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert_eq!(value(&lines, "commands_failed"), Some("0"));
    assert_eq!(value(&lines, "last_command_failure"), None);
}

/// A status with a queue of no threads (a test build that does the I/O
/// on the caller's thread) shows no `io_` key.
#[test]
fn the_report_shows_the_io_queue_only_where_there_are_threads() {
    let mut status = sample();
    status.shown.io_queue = Some(IoQueue::default());
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        5,
    );
    assert!(lines.iter().all(|(key, _)| !key.starts_with("io_")));
}

/// A change of the store's state alone (a new cut) is written as a
/// change of the counters is: within the interval, with no other post.
#[test]
fn a_change_of_the_store_alone_is_written() {
    let board = Arc::new(Board::new());
    let (wrote, written) = mpsc::channel::<StoreShown>();
    let publisher = std::thread::spawn({
        let board = board.clone();
        move || {
            board.publish_with_store(Pace::new(20), |_, shown, _| {
                wrote.send(*shown).map_err(|_| io::Error::other("gone"))
            })
        }
    });
    let wait = Duration::from_secs(20);
    // The first write: nothing was posted yet.
    assert_eq!(written.recv_timeout(wait), Ok(StoreShown::default()));
    let cut = StoreShown {
        cuts: Some(Cuts {
            count: 1,
            newest: Some(Cut {
                number: 1,
                group: 9,
                itself: false,
            }),
        }),
        ..StoreShown::default()
    };
    board.post_store(cut);
    assert_eq!(written.recv_timeout(wait), Ok(cut));
    // Posting the same state again writes nothing.
    board.post_store(cut);
    assert!(written.recv_timeout(Duration::from_millis(100)).is_err());
    board.stop();
    assert_eq!(publisher.join().expect("the publisher"), 0);
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
    // The pump's times are between the cuts and the I/O threads, in the
    // order of `PUMP_KEYS`; the other keys are in this order.
    let at = keys.iter().position(|k| *k == "cut_opens").unwrap() + 1;
    assert_eq!(keys[at..at + PUMP_FIELDS], PUMP_KEYS);
    let keys: Vec<&str> = keys
        .into_iter()
        .filter(|k| !k.starts_with("pump_"))
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
            "commands_aborted",
            "commands_failed",
            "last_command_failure",
            "first_write_copies",
            "status_write_failures",
            "age_ms",
            "status_version",
            "start_ms",
            "zeroes_applied",
            "cuts",
            "cut_group",
            "cut_opens",
            "io_threads",
            "io_in_flight",
            "io_waiting",
            "io_waited",
            "io_in_flight_max",
            "io_waiting_max",
            "cut_state",
            "cuts_held",
            "cut_copied_bytes",
            "cut_bound_bytes",
            "cuts_abandoned",
            "cuts_replaced",
            "cut_holes",
            "cut_hole_chunks",
            "intent_state",
            "intent_region_bytes",
            "intent_regions",
            "intent_named",
            "intent_named_at_start",
            "intent_syncs",
            "intent_sync_ns",
            "intent_sync_max_ns",
            "intent_waits",
            "intent_header_sync",
            "intent_header_syncs",
            "intent_header_sync_ns",
            "intent_header_sync_max_ns",
            // The upload (v15), after the intent map.
            "upload",
            "upload_state",
            "upload_cut",
            "upload_wait_ms",
            "upload_why",
            "lease",
            "lease_holder",
            "lease_epoch",
            "lease_expires_unix_ms",
            "uploaded",
            "uploaded_whole",
            "upload_blocks",
            "upload_found_committed",
            "upload_committed_gone",
            "upload_committed_named",
            "upload_committed_kept",
            "upload_mismatches",
            "upload_abandoned_at_mismatch",
            "upload_cuts_lost",
            "upload_given_up",
            "upload_errors",
            "upload_zero_blocks",
            "upload_checked",
            "upload_rule",
            "upload_last_error",
            "upload_not_offered",
            "upload_cuts_lost_whole",
            "publish_accepted",
            "publish_published",
            "publish_found",
            "publish_refused",
            "publish_expired",
            "publish_withdrawn",
            "publish_pending",
            "publish_oldest",
            "publish_oldest_ms",
            "publish_last",
            "publish_last_cut",
            "publish_last_group",
            "control",
            "control_answered",
            "control_peer",
            "control_malformed",
            "pid",
            "counters"
        ]
    );
    assert!(!has_log_store_keys(&lines));
    assert_eq!(value(&lines, "io_threads"), Some("20"));
    assert_eq!(value(&lines, "io_waiting_max"), Some("25"));
    // The process to signal: only while the kernel shows a live server.
    assert_eq!(value(&lines, "pid"), Some("16909060"));
    assert_eq!(value(&lines, "state"), Some("serving"));
    assert_eq!(value(&lines, "device"), Some("/dev/ublkb4"));
    // The device's own values, not the file's.
    assert_eq!(value(&lines, "queues"), Some("2"));
    assert_eq!(value(&lines, "depth"), Some("32"));
    assert_eq!(value(&lines, "reads"), Some("2"));
    assert_eq!(value(&lines, "held"), Some("8"));
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
        assert_eq!(value(&lines, "failure"), Some("the log store's log failed"));
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

/// `status` says which version of the file it read (`status_version`):
/// a script tells by it a field an older file did not have from a zero
/// the daemon counted.
#[test]
fn status_prints_the_version_of_the_file_it_read() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut nineteenth = encode(&sample()).to_vec();
    for (version, magic) in [
        ("8", b"SKYZSTA8"),
        ("9", b"SKYZSTA9"),
        ("10", b"SKYZST10"),
        ("11", b"SKYZST11"),
        ("12", b"SKYZST12"),
        ("13", b"SKYZST13"),
        ("14", b"SKYZST14"),
        ("15", b"SKYZST15"),
        ("16", b"SKYZST16"),
        ("17", b"SKYZST17"),
        ("18", b"SKYZST18"),
        ("19", b"SKYZST19"),
    ] {
        let mut bytes = match version {
            "8" => tenth_file()[..1960].to_vec(),
            "9" => tenth_file()[..2008].to_vec(),
            "10" => tenth_file(),
            "11" => eleventh_file(),
            "12" => twelfth_file(),
            "13" => thirteenth_file(),
            "14" => fourteenth_file(),
            "15" => fifteenth_file(),
            "16" => sixteenth_file(),
            "17" => seventeenth_file(),
            "18" => eighteenth_file(),
            _ => std::mem::take(&mut nineteenth),
        };
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
    assert_eq!(VERSION, 19);
}

/// A post wakes the publishing thread only when it waits for a change.
/// While it waits for the end of an interval, a post leaves it be, and
/// the write at that end takes what is posted by then: the file is no
/// staler than before, and the last posted counters are written. The
/// queue's thread posts after every turn, tens of thousands of times a
/// second under load, and the wakes, not the writes, were what the
/// status thread spent its time on (measured: 95 ms of CPU a second
/// with a post every 20 µs, of which 9 ms in the writes). Mutant: every
/// post wakes it.
#[test]
fn a_post_wakes_the_publisher_only_when_it_waits_for_a_change() {
    const POSTS: u64 = 20_000;
    fn scenario(mutants: u32) -> Result<(), String> {
        let board = Arc::new(Board::new());
        let written = Arc::new(Mutex::new(Vec::new()));
        let thread = std::thread::spawn({
            let (board, written) = (board.clone(), written.clone());
            move || {
                board.publish(Pace::new(50).with(mutants), |counters, _| {
                    written.lock().expect("lock").push(counters.reads);
                    Ok(())
                })
            }
        });
        assert!(board.first_write_tried(Duration::from_secs(20)));
        // Posts for longer than a few intervals, each of new counters,
        // with the publishing thread in its waits meanwhile.
        for reads in 1..=POSTS {
            board.post(counters(reads, 0));
            let spin = Instant::now();
            while spin.elapsed() < Duration::from_micros(10) {
                std::hint::spin_loop();
            }
        }
        let began = Instant::now();
        while written.lock().expect("lock").last() != Some(&POSTS) {
            if began.elapsed() > Duration::from_secs(20) {
                board.stop();
                return Err("the last counters were not written".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let (wakes, attempts) = board.wakes_and_attempts();
        board.stop();
        let failures = thread.join().expect("the publishing thread");
        // Each write ends a wait for the end of an interval, which a
        // wait for a change may come before; a timed wait may also end
        // a little early and be waited again.
        if wakes > 3 * attempts + 3 {
            return Err(format!(
                "{wakes} wakes for {attempts} writes of {POSTS} posts"
            ));
        }
        if failures != 0 {
            return Err(format!("{failures} failures"));
        }
        Ok(())
    }
    assert_eq!(scenario(0), Ok(()));
    assert!(
        scenario(Mutants::WOKEN_BY_EVERY_POST).is_err(),
        "every post"
    );
}

/// A write asked for again wakes the publishing thread also while it
/// waits out its interval, and is made then, not at the interval's end.
/// The interval is an hour, so a write seen within the test is before
/// its end, and the test rests on events: the thread is seen waiting
/// for a change; a post of new counters wakes it (a wake counted under
/// the board's lock, which it lets go only in its next wait, now one
/// for the interval's end); a write is asked for again; the write is
/// seen. Before that write the only bound is the test's limit of 20 s
/// against the hour. Planted by hand: with `write_again` waking the
/// thread only when it waits for a change, the test fails.
#[test]
fn a_write_asked_for_again_is_made_within_an_interval() {
    fn until(what: &str, done: impl Fn() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(began.elapsed() < Duration::from_secs(20), "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let board = Arc::new(Board::new());
    let written = Arc::new(Mutex::new(Vec::new()));
    let thread = std::thread::spawn({
        let (board, written) = (board.clone(), written.clone());
        move || {
            board.publish(Pace::new(3_600_000), |counters, _| {
                written.lock().expect("lock").push(counters.reads);
                Ok(())
            })
        }
    });
    assert!(board.first_write_tried(Duration::from_secs(20)));
    until("waits for a change", || board.awaits_change());
    let (wakes, _) = board.wakes_and_attempts();
    board.post(counters(1, 0));
    until("woken by the change", || {
        board.wakes_and_attempts().0 > wakes
    });
    assert!(!board.awaits_change(), "waits for the interval's end");
    board.write_again();
    until("the write asked for again", || {
        written.lock().expect("lock").last() == Some(&1)
    });
    assert_eq!(
        board.wakes_and_attempts().1,
        2,
        "the first write and this one"
    );
    board.stop();
    assert_eq!(thread.join().expect("the publishing thread"), 0);
}

/// A newest cut that opens only with a rewind goes through the file as
/// one: written as 2, and read back as not itself, as is any value but
/// 1 (the review of #95, its S1, whose reproducer this is). Fails where both answers are written as 1 (plant T1) and where
/// any value but 0 reads as itself (plant T2).
#[test]
fn a_cut_that_opens_only_with_a_rewind_goes_through_the_file_as_one() {
    let mut status = sample();
    if let Some(Cuts {
        newest: Some(cut), ..
    }) = status.shown.cuts.as_mut()
    {
        cut.itself = false;
    }
    let bytes = encode(&status);
    assert_eq!(u64_at(&bytes, 304), 2);
    let itself = |bytes: &[u8]| {
        decode(bytes)
            .expect("a status")
            .shown
            .cuts
            .and_then(|cuts| cuts.newest)
            .map(|cut| cut.itself)
    };
    assert_eq!(itself(&bytes), Some(false));
    let mut other = bytes;
    other[304..312].copy_from_slice(&3u64.to_le_bytes());
    assert_eq!(itself(&other), Some(false));
}
