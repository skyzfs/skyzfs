use super::*;
use crate::ack::AckFile;

const BOOT: [u8; 16] = [7; 16];

fn attachment(number: u32) -> Attachment {
    Attachment {
        number,
        nonce: [0xA5; 16],
        boot: BOOT,
        shape: Shape {
            size: 0x0102_0304_0506_0708,
            queues: 0x1112,
            depth: 0x2122,
            flags: 0x3132_3334_3536_3738,
        },
    }
}

struct Dirs {
    _keep: tempfile::TempDir,
    volume: PathBuf,
    run: PathBuf,
}

fn dirs() -> Dirs {
    let keep = tempfile::tempdir().expect("a directory");
    let (volume, run) = (keep.path().join("volume"), keep.path().join("run"));
    fs::create_dir(&volume).expect("volume");
    fs::create_dir(&run).expect("run");
    Dirs {
        _keep: keep,
        volume,
        run,
    }
}

fn files(dirs: &Dirs, mutants: u32) -> Files {
    let files = Files::new(&dirs.volume, &dirs.run, BOOT, Duration::ZERO)
        .expect("the volume's directory")
        .with(mutants);
    files.create_run_dirs().expect("run directories");
    files
}

#[test]
fn the_attachment_record_has_the_documented_layout() {
    let bytes = encode_attachment(&attachment(0x4142_4344));
    let mut expected = Vec::new();
    expected.extend(b"SKYZATT1");
    expected.extend([0x44, 0x43, 0x42, 0x41]);
    expected.extend([0x12, 0x11, 0x22, 0x21]);
    expected.extend([0xA5; 16]);
    expected.extend([7; 16]);
    expected.extend([8, 7, 6, 5, 4, 3, 2, 1]);
    expected.extend([0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31]);
    assert_eq!(bytes.to_vec(), expected);
    assert_eq!(decode_attachment(&bytes), Some(attachment(0x4142_4344)));
}

#[test]
fn the_note_has_the_documented_layout() {
    let bytes = encode_note(0x4142_4344, &[0xA5; 16]);
    let mut expected = Vec::new();
    expected.extend(b"SKYZNOT1");
    expected.extend([0x44, 0x43, 0x42, 0x41, 0, 0, 0, 0]);
    expected.extend([0xA5; 16]);
    assert_eq!(bytes.to_vec(), expected);
}

#[test]
fn what_is_written_is_read_and_what_is_absent_is_none() {
    let dirs = dirs();
    let mut files = files(&dirs, 0);
    assert_eq!(files.attachment().expect("read"), None);
    assert_eq!(files.note(3).expect("read"), None);
    assert_eq!(files.acknowledged().expect("read"), None);

    files.write_attachment(&attachment(3)).expect("write");
    files.write_note(3, &[0xA5; 16]).expect("write");
    assert_eq!(files.attachment().expect("read"), Some(attachment(3)));
    assert_eq!(
        read_attachment(&dirs.volume).expect("read"),
        Some(attachment(3))
    );
    assert_eq!(files.note(3).expect("read"), Some([0xA5; 16]));
    assert_eq!(read_note(&dirs.run, 3).expect("read"), Some([0xA5; 16]));
    assert_eq!(files.note(4).expect("read"), None);
    files.remove_note(4).expect("nothing to remove");
    files.remove_note(3).expect("remove");
    assert_eq!(files.note(3).expect("read"), None);

    // A record replaces the one before it, and leaves nothing beside.
    files.write_attachment(&attachment(5)).expect("write");
    assert_eq!(files.attachment().expect("read"), Some(attachment(5)));
    let left: Vec<_> = fs::read_dir(&dirs.volume)
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(left, [std::ffi::OsString::from(ATTACHMENT)]);
}

/// Every prefix of a record, and a record with one byte changed in its
/// fixed part, is an error.
fn torn(mutants: u32) -> Result<(), String> {
    let dirs = dirs();
    let mut files = files(&dirs, mutants);
    let whole = encode_attachment(&attachment(3));
    for len in 0..whole.len() {
        fs::write(dirs.volume.join(ATTACHMENT), &whole[..len]).expect("write");
        if files.attachment().is_ok() {
            return Err(format!("{len} bytes of an attachment record were read"));
        }
    }
    let whole = encode_note(3, &[0xA5; 16]);
    for len in 0..whole.len() {
        fs::write(note_path(&dirs.run, 3), &whole[..len]).expect("write");
        if files.note(3).is_ok() {
            return Err(format!("{len} bytes of a note were read"));
        }
    }
    for at in [0, 7, 12, 15] {
        let mut changed = whole;
        changed[at] ^= 1;
        fs::write(note_path(&dirs.run, 3), changed).expect("write");
        if files.note(3).is_ok() {
            return Err(format!("a note with byte {at} changed was read"));
        }
    }
    Ok(())
}

#[test]
fn a_file_that_is_not_a_whole_record_is_an_error_and_not_no_record() {
    assert_eq!(torn(0), Ok(()));
    assert!(torn(Mutants::TORN_IS_NONE).is_err());
}

fn another_devices_note(mutants: u32) -> Result<(), String> {
    let dirs = dirs();
    let mut files = files(&dirs, mutants);
    files.write_note(3, &[0xA5; 16]).expect("write");
    // The note of device 3 turns up under device 4's name.
    fs::copy(note_path(&dirs.run, 3), note_path(&dirs.run, 4)).expect("copy");
    match files.note(4) {
        Err(_) => Ok(()),
        Ok(read) => Err(format!("device 4 was given {read:?}")),
    }
}

#[test]
fn a_note_counts_only_for_the_device_it_names() {
    assert_eq!(another_devices_note(0), Ok(()));
    assert!(another_devices_note(Mutants::ANY_NUMBER).is_err());
}

/// While a record is being replaced, a reader finds the old record or
/// the new one: the file at the record's name is never written to.
fn replaced_whole(mutants: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let dirs = dirs();
    let mut files = files(&dirs, mutants);
    files.write_attachment(&attachment(3)).expect("write");
    files.write_note(3, &[1; 16]).expect("write");
    let paths = [dirs.volume.join(ATTACHMENT), note_path(&dirs.run, 3)];
    let before: Vec<u64> = paths
        .iter()
        .map(|p| fs::metadata(p).expect("there").ino())
        .collect();
    // Held open across the replacement, the old files keep their bytes.
    let old: Vec<Vec<u8>> = paths.iter().map(|p| fs::read(p).expect("read")).collect();
    let held: Vec<fs::File> = paths
        .iter()
        .map(|p| fs::File::open(p).expect("open"))
        .collect();
    files.write_attachment(&attachment(4)).expect("write");
    files.write_note(3, &[2; 16]).expect("write");
    for (((path, before), old), mut held) in paths.iter().zip(before).zip(old).zip(held) {
        if fs::metadata(path).expect("there").ino() == before {
            return Err(format!("{} was written in place", path.display()));
        }
        let mut still = Vec::new();
        held.read_to_end(&mut still).expect("read");
        if still != old {
            return Err(format!("the old {} changed", path.display()));
        }
    }
    Ok(())
}

#[test]
fn a_record_is_replaced_whole_and_never_written_in_place() {
    assert_eq!(replaced_whole(0), Ok(()));
    assert!(replaced_whole(Mutants::IN_PLACE).is_err());
}

#[test]
fn what_was_acknowledged_counts_only_in_the_boot_it_was_recorded_in() {
    let dirs = dirs();
    let mut here = files(&dirs, 0);
    let path = here.acknowledged_path();

    // An empty record: nothing to reach.
    let mut ack = AckFile::open(&path, [3; 16], BOOT).expect("open");
    assert_eq!(here.acknowledged().expect("read"), None);

    ack.advance(4096).expect("advance");
    assert_eq!(
        here.acknowledged().expect("read"),
        Some(MustReach {
            identity: [3; 16],
            position: 4096,
        })
    );

    // Another boot: no client holds a device across a reboot.
    let mut later =
        Files::new(&dirs.volume, &dirs.run, [8; 16], Duration::ZERO).expect("the volume");
    assert_eq!(later.acknowledged().expect("read"), None);

    // Not a whole record: an error, which the lifecycle takes as a
    // volume that failed while held.
    fs::write(&path, b"SKYZACK1").expect("write");
    assert!(here.acknowledged().is_err());
}

#[test]
fn the_boot_id_is_the_kernels_uuid() {
    assert_eq!(
        parse_boot_id("00010203-0405-0607-0809-0a0b0c0d0e0f\n"),
        Some([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
    );
    for wrong in [
        "",
        "0001",
        "00010203-0405-0607-0809-0a0b0c0d0e0g",
        "+0010203-0405-0607-0809-0a0b0c0d0e0f",
    ] {
        assert_eq!(parse_boot_id(wrong), None, "{wrong:?}");
    }
    if cfg!(target_os = "linux") {
        assert_eq!(boot_id().expect("boot id"), boot_id().expect("boot id"));
    }
}

#[test]
fn nonces_differ() {
    let dirs = dirs();
    let mut files = files(&dirs, 0);
    assert_ne!(files.nonce().expect("nonce"), files.nonce().expect("nonce"));
}

/// Two volumes, one run directory: each sees only its own position.
fn two_volumes(mutants: u32) -> Result<(), String> {
    let dirs = dirs();
    let other = dirs.volume.with_file_name("other");
    fs::create_dir(&other).expect("volume");
    let mut first = files(&dirs, mutants);
    let mut second = Files::new(&other, &dirs.run, BOOT, Duration::ZERO)
        .expect("the volume")
        .with(mutants);
    second.create_run_dirs().expect("run directories");

    let mut ack = AckFile::open(&first.acknowledged_path(), [1; 16], BOOT).expect("open");
    ack.advance(100).expect("advance");
    let mut ack = AckFile::open(&second.acknowledged_path(), [2; 16], BOOT).expect("open");
    ack.advance(200).expect("advance");

    for (files, identity, position) in [(&mut first, [1; 16], 100), (&mut second, [2; 16], 200)] {
        let read = files.acknowledged().map_err(|e| e.to_string())?;
        if read != Some(MustReach { identity, position }) {
            return Err(format!("the volume at {position} was given {read:?}"));
        }
    }
    Ok(())
}

#[test]
fn what_belongs_to_a_volume_in_a_shared_run_directory_is_under_the_volumes_own_name() {
    assert_eq!(two_volumes(0), Ok(()));
    assert!(two_volumes(Mutants::ACK_AT_TOP).is_err());

    use std::os::unix::fs::MetadataExt;
    let dirs = dirs();
    let files = files(&dirs, 0);
    let meta = fs::metadata(&dirs.volume).expect("there");
    assert_eq!(
        files.acknowledged_path(),
        dirs.run
            .join(format!("v-{:x}-{:x}", meta.dev(), meta.ino()))
            .join("acknowledged")
    );
    // The notes of devices are not per volume: a number is one device
    // on the machine.
    assert_eq!(note_path(&dirs.run, 7), dirs.run.join("ublk7.note"));
}
