//! `status` and `detach` against a kernel of the test's own, with the
//! real records, the real status file and a real store.

use std::collections::BTreeMap;
use std::path::PathBuf;

use skyzfs_blk::Base;
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::store::{Store, StoreConfig};
use skyzfs_ublk::lifecycle::{
    AddError, Attachment, DeviceInfo, DeviceState, Failure, Held, Reason, Records, Shape,
};
use skyzfs_ublk::pump::Counters;
use skyzfs_ublk::status::Status;

use super::*;

const BOOT: [u8; 16] = [7; 16];
const NONCE: [u8; 16] = [0xA5; 16];
const SHAPE: Shape = Shape {
    size: 8 << 20,
    queues: 1,
    depth: 64,
    flags: 0x18,
};

/// A kernel that keeps devices. A device in `held` keeps its number
/// when it is deleted.
#[derive(Default)]
struct Kernel {
    devices: BTreeMap<u32, DeviceInfo>,
    held: Vec<u32>,
    deleted: Vec<u32>,
    /// The kernel's release; 7.0.0 when `None`.
    release: Option<String>,
}

impl Control for Kernel {
    fn features(&mut self) -> io::Result<Option<u64>> {
        Ok(Some(u64::MAX))
    }
    fn release(&mut self) -> io::Result<String> {
        Ok(self.release.clone().unwrap_or_else(|| "7.0.0".to_string()))
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        Ok(self.devices.get(&number).copied())
    }
    fn add(&mut self, _number: u32, _shape: &Shape) -> Result<(), AddError> {
        Err(AddError::Other(io::Error::other("not in these tests")))
    }
    fn disown(&mut self, _number: u32) {}
    fn delete(&mut self, number: u32) -> io::Result<()> {
        self.deleted.push(number);
        if self.held.contains(&number) {
            if let Some(device) = self.devices.get_mut(&number) {
                device.state = DeviceState::Dead;
            }
        } else {
            self.devices.remove(&number);
        }
        Ok(())
    }
    fn begin_recovery(&mut self, _number: u32) -> io::Result<()> {
        Err(io::Error::other("not in these tests"))
    }
    fn client_holds(&mut self, _number: u32) -> Option<bool> {
        None
    }
}

struct Setup {
    _keep: tempfile::TempDir,
    volume: Volume,
    kernel: Kernel,
}

impl Setup {
    /// A volume, and nothing attached.
    fn new() -> Self {
        let keep = tempfile::tempdir().expect("a directory");
        let (dir, run_dir) = (keep.path().join("volume"), keep.path().join("run"));
        std::fs::create_dir(&dir).expect("the volume's directory");
        std::fs::create_dir(&run_dir).expect("the run directory");
        let config = StoreConfig {
            volume: *b"a volume on disk",
            size_mib: 8,
            base: Base::Empty,
            align: 4096,
            segment_size: 8 << 20,
            write_unit: Identity::system_write_unit(),
        };
        let store = Store::create(
            FsDir::new(&dir).expect("the directory"),
            &config,
            OpenOptions::default(),
        )
        .expect("create");
        drop(store);
        Self {
            _keep: keep,
            volume: Volume { dir, run_dir },
            kernel: Kernel::default(),
        }
    }

    fn records(&self) -> Files {
        Files::new(&self.volume.dir, &self.volume.run_dir, BOOT, Duration::ZERO).expect("records")
    }

    /// A daemon of this boot attached device 4, and the kernel has it
    /// in `state`.
    fn attached(mut self, state: DeviceState) -> Self {
        let mut records = self.records();
        records.create_run_dirs().expect("the run directories");
        records
            .write_attachment(&Attachment {
                number: 4,
                nonce: NONCE,
                boot: BOOT,
                shape: SHAPE,
            })
            .expect("the record");
        records.write_note(4, &NONCE).expect("the note");
        self.kernel.devices.insert(
            4,
            DeviceInfo {
                state,
                shape: SHAPE,
            },
        );
        self
    }

    /// A status file's content for this volume: [`sample`] with the
    /// store's identity.
    fn sample(&self) -> Status {
        let dir = FsDir::new(&self.volume.dir).expect("the directory");
        Status {
            identity: Store::identity_of(&dir)
                .expect("read")
                .expect("a superblock"),
            ..sample()
        }
    }

    fn status_path(&self) -> PathBuf {
        self.records().volume_run_dir().join(status::STATUS)
    }

    fn status(&mut self) -> Output {
        let now = Now {
            boot: BOOT,
            uptime_ms: 10_250,
        };
        status(&self.volume, &mut self.kernel, &now)
    }

    fn detach(&mut self) -> Output {
        detach(&self.volume, false, &mut self.kernel, BOOT)
    }
}

fn value<'a>(out: &'a str, key: &str) -> Option<&'a str> {
    out.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
}

fn sample() -> Status {
    Status {
        version: skyzfs_ublk::status::VERSION,
        pid: 4242,
        queues: 1,
        depth: 64,
        boot: BOOT,
        identity: [3; 16],
        written_ms: 10_000,
        counters: Counters {
            reads: 5,
            writes: 6,
            flushes: 7,
            discards: 8,
            write_zeroes: 9,
            refused: 0,
            in_flight: 2,
            unstored: 0,
        },
        failures: 0,
        first_write_copies: 0,
        recovered_unacknowledged: 0,
        log: None,
    }
}

#[test]
fn a_directory_that_is_no_volume_is_an_error_and_gets_no_lock_file() {
    let keep = tempfile::tempdir().expect("a directory");
    let volume = Volume {
        dir: keep.path().join("empty"),
        run_dir: keep.path().join("run"),
    };
    std::fs::create_dir(&volume.dir).expect("the directory");
    let mut kernel = Kernel::default();
    let now = Now {
        boot: BOOT,
        uptime_ms: 0,
    };
    for output in [
        status(&volume, &mut kernel, &now),
        detach(&volume, false, &mut kernel, BOOT),
    ] {
        assert_eq!(output.exit, Exit::Error);
        assert!(output.out.is_empty());
        assert!(output.err.contains("no volume in"), "{}", output.err);
    }
    let left = std::fs::read_dir(&volume.dir).expect("read").count();
    assert_eq!(
        left, 0,
        "something was made in a directory that is no volume"
    );
}

#[test]
fn status_prints_one_key_and_value_per_line_as_the_rig_reads_them() {
    // Nothing attached.
    let mut setup = Setup::new();
    let output = setup.status();
    assert_eq!((output.exit, output.err.as_str()), (Exit::Done, ""));
    assert_eq!(output.out, "state detached\n");

    // Served: the rig's keys, each a line of its own, with the value
    // behind one blank.
    let mut setup = Setup::new().attached(DeviceState::Live);
    status::write(&setup.status_path(), &setup.sample()).expect("the status file");
    let output = setup.status();
    assert_eq!((output.exit, output.err.as_str()), (Exit::Done, ""));
    for (key, wanted) in [
        ("state", "serving"),
        ("device", "/dev/ublkb4"),
        ("queues", "1"),
        ("depth", "64"),
        ("reads", "5"),
        ("writes", "6"),
        ("flushes", "7"),
        ("discards", "8"),
        ("write_zeroes", "9"),
        ("held", "2"),
        ("unstored", "0"),
        ("first_write_copies", "0"),
        ("recovered_unacknowledged", "0"),
        ("pid", "4242"),
        ("age_ms", "250"),
    ] {
        assert_eq!(value(&output.out, key), Some(wanted), "{key}");
    }

    // The server is gone: no pid to kill, and the counters are said to
    // be left behind.
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    status::write(&setup.status_path(), &setup.sample()).expect("the status file");
    let output = setup.status();
    assert_eq!(value(&output.out, "state"), Some("waiting for a server"));
    assert_eq!(value(&output.out, "pid"), None);
    assert_eq!(value(&output.out, "reads"), Some("5"));
}

#[test]
fn status_shows_a_failure_of_this_boot_and_says_when_its_file_cannot_be_read() {
    let mut setup = Setup::new().attached(DeviceState::Dead);
    setup
        .records()
        .write_failure(&Failure {
            held: Held::Yes,
            reason: Reason::LogFailed,
            identity: [3; 16],
        })
        .expect("the failure note");
    let output = setup.status();
    assert_eq!(output.exit, Exit::Done);
    assert_eq!(
        value(&output.out, "state"),
        Some("failed, held by a client: not served again in this boot")
    );
    assert_eq!(value(&output.out, "failure_held"), Some("yes"));

    // A status file that is not whole: said, exit 1, and the state is
    // printed all the same.
    std::fs::write(setup.status_path(), b"half").expect("write");
    let output = setup.status();
    assert_eq!(output.exit, Exit::Error);
    assert!(output.err.contains("the status file cannot be read"));
    assert!(value(&output.out, "state").is_some());
    assert_eq!(value(&output.out, "reads"), None);

    // A failure note that is not whole counts as held.
    std::fs::remove_file(setup.status_path()).expect("remove");
    std::fs::write(setup.records().failure_path(), b"half").expect("write");
    let output = setup.status();
    assert_eq!(
        value(&output.out, "state"),
        Some("failed, held by a client: not served again in this boot")
    );
}

/// On a kernel below 6.9 a detach refuses before it looks at anything,
/// with status 1 and a message that names the asynchronous delete and
/// the override; with the override it deletes.
#[test]
fn detach_on_a_kernel_without_the_asynchronous_delete_is_refused_unless_asserted() {
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    setup.kernel.release = Some("6.8.0-45-generic".to_string());
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Error);
    assert!(output.out.is_empty());
    for said in [
        "not detached",
        "UBLK_U_CMD_DEL_DEV_ASYNC",
        "Linux 6.9",
        "--kernel-has-async-delete",
        "Nothing was done",
    ] {
        assert!(output.err.contains(said), "{said}: {}", output.err);
    }
    assert!(setup.kernel.deleted.is_empty());
    assert!(setup.kernel.devices.contains_key(&4));
    assert!(setup.records().attachment().expect("read").is_some());

    let output = detach(&setup.volume, true, &mut setup.kernel, BOOT);
    assert_eq!(output.exit, Exit::Done, "{}", output.err);
    assert_eq!(output.out, "deleted /dev/ublkb4\n");
}

#[test]
fn detach_exits_and_speaks_as_the_rig_expects_for_each_thing_it_finds() {
    // Nothing: 0, on standard output.
    let mut setup = Setup::new();
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Done);
    assert!(output.out.starts_with("nothing to detach") && output.err.is_empty());

    // A waiting device and one that was never started: deleted, 0.
    for state in [DeviceState::Waiting, DeviceState::Dead] {
        let mut setup = Setup::new().attached(state);
        let output = setup.detach();
        assert_eq!(output.exit, Exit::Done);
        assert_eq!(
            (output.out.as_str(), output.err.as_str()),
            ("deleted /dev/ublkb4\n", "")
        );
        assert!(setup.kernel.devices.is_empty());
        assert_eq!(setup.records().attachment().expect("read"), None);
        // And again: nothing is left to do.
        assert!(setup.detach().out.starts_with("nothing to detach"));
    }

    // Deleted and still held: 6, and said on standard error. A script
    // can tell it from "nothing of this volume is left attached".
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    setup.kernel.held.push(4);
    let output = setup.detach();
    assert_eq!(output.exit, Exit::DeletedHeld);
    assert_eq!(output.exit.status(), 6);
    assert!(output.out.is_empty());
    assert!(output.err.contains("still holds it open"), "{}", output.err);
    assert_eq!(setup.kernel.deleted, [4]);

    // Deleted before and still held looks like never started: it is
    // deleted again, which does no harm, and said to be held.
    let mut setup = Setup::new().attached(DeviceState::Dead);
    setup.kernel.held.push(4);
    let output = setup.detach();
    assert_eq!(output.exit, Exit::DeletedHeld);
    assert!(output.err.contains("still holds it open"), "{}", output.err);
    assert_eq!(setup.kernel.deleted, [4]);

    // No server, no note, the recorded values: deleted before and
    // still held, or another's. Whose it is cannot be told: 1, said in
    // its own words, nothing done, the record stays.
    let mut setup = Setup::new().attached(DeviceState::Dead);
    setup.records().remove_note(4).expect("the note");
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Error);
    assert!(output.out.is_empty());
    assert!(
        output.err.contains("has no server and no note"),
        "{}",
        output.err
    );
    assert!(setup.kernel.deleted.is_empty());
    assert!(setup.records().attachment().expect("read").is_some());

    // Not known to be this volume's: 1, nothing done.
    for (nonce, said) in [
        (Some([0x5A; 16]), "is not this volume's"),
        (None, "has this volume's values and no note"),
    ] {
        let mut setup = Setup::new().attached(DeviceState::Waiting);
        let mut records = setup.records();
        match nonce {
            Some(nonce) => records.write_note(4, &nonce).expect("the note"),
            None => records.remove_note(4).expect("the note"),
        }
        let output = setup.detach();
        assert_eq!(output.exit, Exit::Error);
        assert!(output.err.contains(said), "{}", output.err);
        assert!(output.err.contains("nothing was done"));
        assert!(setup.kernel.deleted.is_empty());
        assert!(setup.records().attachment().expect("read").is_some());
    }

    // Live under a server: 5, nothing done.
    let mut setup = Setup::new().attached(DeviceState::Live);
    let output = setup.detach();
    assert_eq!(output.exit, Exit::RefusedHeld);
    assert!(output.err.contains("a server is live on /dev/ublkb4"));
    assert!(setup.kernel.deleted.is_empty());
}

#[test]
fn detach_of_a_volume_a_daemon_holds_gives_up_after_two_seconds_and_touches_nothing() {
    assert_eq!(LOCK_PAUSE * LOCK_RETRIES, Duration::from_secs(2));
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    let (daemon, _) = Store::open(
        FsDir::new(&setup.volume.dir).expect("the directory"),
        OpenOptions::default(),
    )
    .expect("open");
    let began = std::time::Instant::now();
    let output = setup.detach();
    assert!(began.elapsed() >= Duration::from_secs(2));
    assert_eq!(output.exit, Exit::RefusedHeld);
    assert!(output.err.contains("a daemon holds this volume"));
    assert!(setup.kernel.deleted.is_empty());
    assert!(setup.records().attachment().expect("read").is_some());
    drop(daemon);
    assert_eq!(setup.detach().out, "deleted /dev/ublkb4\n");
}

#[test]
fn detach_leaves_the_failure_note_and_create_warns_of_one_of_this_boot() {
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    let (dir, run_dir) = (setup.volume.dir.clone(), setup.volume.run_dir.clone());
    assert_eq!(create_warning(&dir, &run_dir, &BOOT), None);

    for (held, said) in [
        (false, "--acknowledge-failure"),
        (true, "not served before a reboot"),
    ] {
        setup
            .records()
            .write_failure(&Failure {
                held: held.into(),
                reason: Reason::Short,
                identity: [0; 16],
            })
            .expect("the failure note");
        let warning = create_warning(&dir, &run_dir, &BOOT).expect("a warning");
        assert!(warning.contains(said), "{warning}");
        // Another boot: the note means nothing.
        assert_eq!(create_warning(&dir, &run_dir, &[8; 16]), None);
    }

    let output = setup.detach();
    assert_eq!(output.out, "deleted /dev/ublkb4\n");
    let note = setup.records().failure().expect("read");
    assert_eq!(
        note.map(|f| f.held),
        Some(Held::Yes),
        "detach touched the note"
    );

    // A note that is not whole warns as a held one does.
    std::fs::write(setup.records().failure_path(), b"half").expect("write");
    let warning = create_warning(&dir, &run_dir, &BOOT).expect("a warning");
    assert!(warning.contains("not served before a reboot"));
}

fn creating(dir: &Path, run_dir: &Path, size: u64, base_pattern: Option<u64>) -> Create {
    Create {
        dir: dir.into(),
        size,
        run_dir: run_dir.into(),
        base_pattern,
    }
}

/// `create` makes a volume of the size asked for that the store opens,
/// says its size, its id and its identity in one line, and fixes the
/// alignment, the segment size and the write unit.
#[test]
fn create_makes_a_volume_the_store_opens_and_says_what_it_made() {
    let keep = tempfile::tempdir().expect("a directory");
    let (dir, run_dir) = (keep.path().join("volume"), keep.path().join("run"));
    std::fs::create_dir(&dir).expect("the volume's directory");
    let id = *b"0123456789abcdef";

    let output = create(&creating(&dir, &run_dir, 8 << 20, None), id, Some(BOOT));
    assert_eq!((output.exit, output.err.as_str()), (Exit::Done, ""));

    let (store, _) = Store::open(
        FsDir::new(&dir).expect("the directory"),
        OpenOptions::default(),
    )
    .expect("the store opens what create made");
    let identity = store.identity();
    assert_eq!(
        skyzfs_blk::BlockTarget::geometry(&store).size_bytes(),
        8 << 20
    );
    drop(store);
    assert_eq!(
        output.out,
        format!(
            "created {}: 8388608 bytes, volume 30313233343536373839616263646566, identity {}\n",
            dir.display(),
            hex(&identity)
        )
    );
    // Nothing of the run directory was made.
    assert!(!run_dir.exists());
    // An empty base: the home image is a hole.
    let home = std::fs::read(dir.join(skyzfs_local::store::HOME_NAME)).expect("the home image");
    assert_eq!(home.len(), 8 << 20);
    assert!(home.iter().all(|b| *b == 0));
    // The segment size is the constant.
    assert_eq!((SEGMENT_SIZE, ALIGN), (64 << 20, 4096));

    // And status and detach take it for a volume.
    let volume = Volume {
        dir: dir.clone(),
        run_dir: run_dir.clone(),
    };
    let mut kernel = Kernel::default();
    let now = Now {
        boot: BOOT,
        uptime_ms: 1,
    };
    assert_eq!(status(&volume, &mut kernel, &now).exit, Exit::Done);
}

/// `create` makes a missing directory (its last part, mode 0700), and
/// refuses a missing parent, a path that is not a directory, and a
/// directory that holds a volume: status 1, and nothing changes.
#[test]
fn create_makes_the_last_part_and_refuses_a_missing_parent_a_file_and_a_volume() {
    use std::os::unix::fs::PermissionsExt;
    let keep = tempfile::tempdir().expect("a directory");
    let run_dir = keep.path().join("run");

    let deep = keep.path().join("missing").join("volume");
    let output = create(
        &creating(&deep, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!((output.exit, output.out.as_str()), (Exit::Error, ""));
    assert!(output.err.contains("parent"), "{}", output.err);
    assert!(!keep.path().join("missing").exists());

    let made = keep.path().join("made");
    let output = create(
        &creating(&made, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Done, "{}", output.err);
    let mode = std::fs::metadata(&made).expect("made").permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(Store::open(FsDir::new(&made).expect("dir"), OpenOptions::default()).is_ok());

    let dir = keep.path().join("volume");

    // A file is no directory either.
    std::fs::write(&dir, b"a file").expect("write");
    let output = create(
        &creating(&dir, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Error);
    assert_eq!(std::fs::read(&dir).expect("read"), b"a file");
    std::fs::remove_file(&dir).expect("remove");

    std::fs::create_dir(&dir).expect("the volume's directory");
    let first = create(
        &creating(&dir, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(first.exit, Exit::Done);
    let before = Store::open(FsDir::new(&dir).expect("dir"), OpenOptions::default())
        .expect("open")
        .0
        .identity();
    let again = create(
        &creating(&dir, &run_dir, 2 << 20, None),
        [2; 16],
        Some(BOOT),
    );
    assert_eq!((again.exit, again.out.as_str()), (Exit::Error, ""));
    assert!(
        again.err.contains("holds a volume already"),
        "{}",
        again.err
    );
    let (store, _) =
        Store::open(FsDir::new(&dir).expect("dir"), OpenOptions::default()).expect("open");
    assert_eq!(store.identity(), before);
    assert_eq!(
        skyzfs_blk::BlockTarget::geometry(&store).size_bytes(),
        1 << 20
    );
}

/// The test build's base: every block of the new volume is the
/// pattern's.
#[test]
fn create_with_a_base_pattern_makes_a_volume_that_holds_the_pattern() {
    let keep = tempfile::tempdir().expect("a directory");
    let (dir, run_dir) = (keep.path().join("volume"), keep.path().join("run"));
    std::fs::create_dir(&dir).expect("the volume's directory");
    let output = create(
        &creating(&dir, &run_dir, 1 << 20, Some(47806)),
        [3; 16],
        None,
    );
    assert_eq!(output.exit, Exit::Done);
    let home = std::fs::read(dir.join(skyzfs_local::store::HOME_NAME)).expect("the home image");
    assert_eq!(home.len(), 1 << 20);
    let mut want = vec![0u8; 4096];
    for (block, got) in home.chunks(4096).enumerate() {
        Base::Pattern(47806).fill_block(block as u64, &mut want);
        assert_eq!(got, &want[..], "block {block}");
    }
}

/// A failure note of this boot under the run directory: `create` makes
/// the volume, warns on standard error, and leaves the note. Without
/// the boot id it cannot tell and says nothing.
#[test]
fn create_warns_of_a_failure_note_of_this_boot_and_leaves_it() {
    let setup = Setup::new();
    let (run_dir, old) = (setup.volume.run_dir.clone(), setup.volume.dir.clone());
    setup
        .records()
        .write_failure(&Failure {
            held: Held::No,
            reason: Reason::LogFailed,
            identity: [0; 16],
        })
        .expect("the failure note");
    let note = std::fs::read(setup.records().failure_path()).expect("the note");

    // The volume is made again in the same directory.
    for entry in std::fs::read_dir(&old).expect("list") {
        std::fs::remove_file(entry.expect("entry").path()).expect("remove");
    }
    let output = create(
        &creating(&old, &run_dir, 1 << 20, None),
        [4; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Done);
    assert!(output.out.starts_with("created "), "{}", output.out);
    assert!(
        output.err.starts_with("skyzfsd: warning: "),
        "{}",
        output.err
    );
    assert!(
        output.err.contains("--acknowledge-failure"),
        "{}",
        output.err
    );
    assert_eq!(
        std::fs::read(setup.records().failure_path()).expect("the note"),
        note
    );

    for (boot, entry) in [(None, 5u8), (Some([8; 16]), 6)] {
        for entry in std::fs::read_dir(&old).expect("list") {
            std::fs::remove_file(entry.expect("entry").path()).expect("remove");
        }
        let output = create(&creating(&old, &run_dir, 1 << 20, None), [entry; 16], boot);
        assert_eq!((output.exit, output.err.as_str()), (Exit::Done, ""));
    }
}

/// A create that fails after it made the directory removes it again if
/// it is empty, and leaves one it did not make, or one that is not
/// empty.
#[test]
fn a_failed_create_leaves_no_directory_it_made() {
    let keep = tempfile::tempdir().expect("a directory");
    let fail = |_dir: FsDir| -> Result<(), Output> { Err(Output::error("no")) };

    let made = keep.path().join("made");
    assert!(in_directory(&made, fail).is_err());
    assert!(!made.exists(), "the directory it made is gone");

    let there = keep.path().join("there");
    std::fs::create_dir(&there).expect("dir");
    assert!(in_directory(&there, fail).is_err());
    assert!(there.is_dir(), "a directory it did not make stays");

    let left = keep.path().join("left");
    let put = |dir: FsDir| -> Result<(), Output> {
        std::fs::write(dir.path().join("x"), b"x").expect("write");
        Err(Output::error("no"))
    };
    assert!(in_directory(&left, put).is_err());
    assert!(
        left.join("x").exists(),
        "a directory with something in it stays"
    );

    let fine = keep.path().join("fine");
    assert_eq!(in_directory(&fine, |_| Ok(7)).ok(), Some(7));
    assert!(fine.is_dir());
}

/// A real create that fails after it has made the directory, the lock
/// file and the home image's first file (a size the file system cannot
/// hold) leaves nothing: not the files, not the directory. In a
/// directory that was there before, the directory stays, and the next
/// create works in it.
#[test]
fn a_create_that_fails_half_way_removes_what_it_made() {
    let keep = tempfile::tempdir().expect("a directory");
    let run_dir = keep.path().join("run");
    // Beyond what ext4, XFS and btrfs hold in one file. A file system
    // that takes it (a sparse file on tmpfs) cannot show the failure.
    let too_large = 1u64 << 62;

    let made = keep.path().join("made");
    let output = create(
        &creating(&made, &run_dir, too_large, None),
        [1; 16],
        Some(BOOT),
    );
    if output.exit == Exit::Done {
        eprintln!(
            "{} holds a file of {too_large} bytes: not exercised",
            keep.path().display()
        );
        return;
    }
    assert_eq!((output.exit, output.out.as_str()), (Exit::Error, ""));
    assert!(output.err.contains("creating a volume"), "{}", output.err);
    assert!(!made.exists(), "the directory it made is gone");

    let there = keep.path().join("there");
    std::fs::create_dir(&there).expect("dir");
    let output = create(
        &creating(&there, &run_dir, too_large, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Error);
    assert!(there.is_dir(), "a directory it did not make stays");
    let output = create(
        &creating(&there, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Done, "{}", output.err);
}

/// `create` in a directory that holds files of somebody else, also
/// ones whose names begin like the store's own: refused with status 1,
/// the file named, and the directory exactly as it was, with no lock
/// file added. Once they are gone the same command makes the volume.
#[test]
fn create_refuses_a_directory_with_files_that_are_not_its_own_and_touches_nothing() {
    let keep = tempfile::tempdir().expect("a directory");
    let run_dir = keep.path().join("run");
    let listing = |dir: &Path| {
        let mut names: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .expect("list")
            .map(|entry| {
                let entry = entry.expect("entry");
                let name = entry.file_name().into_string().expect("a name");
                (name, std::fs::read(entry.path()).expect("read"))
            })
            .collect();
        names.sort();
        names
    };
    for strangers in [
        &["home.tar", "log.txt"][..],
        &["home.pre"],
        &["log.000001"],
        &["log.failed"],
        &["notes"],
        &["home.img.bak", "home.new", "log.000000"],
    ] {
        let dir = keep.path().join("volume");
        std::fs::create_dir(&dir).expect("the directory");
        for name in strangers {
            std::fs::write(dir.join(name), name.as_bytes()).expect("write");
        }
        let before = listing(&dir);
        let output = create(
            &creating(&dir, &run_dir, 1 << 20, None),
            [1; 16],
            Some(BOOT),
        );
        assert_eq!(
            (output.exit, output.out.as_str()),
            (Exit::Error, ""),
            "{strangers:?}"
        );
        assert!(
            output.err.contains("Nothing was done"),
            "{strangers:?}: {}",
            output.err
        );
        assert!(
            strangers
                .iter()
                .any(|name| output.err.contains(&format!("`{name}`"))),
            "{strangers:?}: {}",
            output.err
        );
        assert_eq!(listing(&dir), before, "{strangers:?}");
        std::fs::remove_dir_all(&dir).expect("remove");
    }

    // What an interrupted create leaves, under its exact names, is
    // create's own: removed and made again.
    let dir = keep.path().join("volume");
    std::fs::create_dir(&dir).expect("the directory");
    for name in skyzfs_local::store::CREATE_LEFTOVERS {
        std::fs::write(dir.join(name), b"left").expect("write");
    }
    let output = create(
        &creating(&dir, &run_dir, 1 << 20, None),
        [1; 16],
        Some(BOOT),
    );
    assert_eq!(output.exit, Exit::Done, "{}", output.err);
    assert!(Store::open(FsDir::new(&dir).expect("dir"), OpenOptions::default()).is_ok());
}

/// `status` and `detach` open the driver only when they have something
/// to ask it. A volume with no record, or with a record of an earlier
/// boot, is detached whatever the driver is: after a reboot and before
/// the driver is loaded both answer, with status 0, and the driver was
/// not opened. With a record of this boot the driver is asked, and its
/// absence is the error it is.
#[test]
fn status_and_detach_do_not_open_the_driver_when_nothing_is_attached_in_this_boot() {
    use std::cell::Cell;
    let now = Now {
        boot: BOOT,
        uptime_ms: 10_250,
    };
    let earlier_boot = [0xEE; 16];
    for record_of_earlier_boot in [false, true] {
        let setup = Setup::new();
        if record_of_earlier_boot {
            let mut records = setup.records();
            records.create_run_dirs().expect("the run directories");
            records
                .write_attachment(&Attachment {
                    number: 4,
                    nonce: NONCE,
                    boot: earlier_boot,
                    shape: SHAPE,
                })
                .expect("the record");
        }
        let opened = Cell::new(0);
        let mut absent = Lazy::new(|| -> io::Result<Box<dyn Control>> {
            opened.set(opened.get() + 1);
            Err(io::Error::other("no ublk driver"))
        });
        let output = status(&setup.volume, &mut absent, &now);
        assert_eq!(output.exit, Exit::Done, "{}", output.err);
        assert_eq!(value(&output.out, "state"), Some("detached"));
        let output = detach(&setup.volume, false, &mut absent, BOOT);
        assert_eq!(output.exit, Exit::Done, "{}", output.err);
        assert!(
            output.out.starts_with("nothing to detach"),
            "{}",
            output.out
        );
        assert_eq!(opened.get(), 0, "the driver was opened");
    }

    let setup = Setup::new().attached(DeviceState::Waiting);
    let opened = Cell::new(0);
    let mut absent = Lazy::new(|| -> io::Result<Box<dyn Control>> {
        opened.set(opened.get() + 1);
        Err(io::Error::other("no ublk driver"))
    });
    let output = status(&setup.volume, &mut absent, &now);
    assert_eq!(output.exit, Exit::Error);
    assert!(output.err.contains("no ublk driver"), "{}", output.err);
    assert_eq!(opened.get(), 1);

    // And through it a driver that is there answers as itself.
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    let devices = std::mem::take(&mut setup.kernel.devices);
    let mut there = Lazy::new(move || -> io::Result<Box<dyn Control>> {
        Ok(Box::new(Kernel {
            devices: devices.clone(),
            ..Kernel::default()
        }))
    });
    let output = status(&setup.volume, &mut there, &now);
    assert_eq!(output.exit, Exit::Done, "{}", output.err);
    assert_eq!(value(&output.out, "state"), Some("waiting for a server"));
}

/// A status file left in the run directory by a volume that was in the
/// same directory before (made again since) is not this volume's: its
/// counters are not shown, and nothing is wrong. The file of this
/// volume is shown.
#[test]
fn status_does_not_show_the_status_file_of_another_volume_in_the_same_directory() {
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    let own = setup.sample();
    assert_ne!(own.identity, sample().identity);
    // The store's identity is what an open store says it is.
    let (store, _) = Store::open(
        FsDir::new(&setup.volume.dir).expect("dir"),
        OpenOptions::default(),
    )
    .expect("open");
    assert_eq!(store.identity(), own.identity);
    drop(store);

    status::write(&setup.status_path(), &sample()).expect("the status file");
    let output = setup.status();
    assert_eq!((output.exit, output.err.as_str()), (Exit::Done, ""));
    assert_eq!(value(&output.out, "state"), Some("waiting for a server"));
    assert_eq!(value(&output.out, "reads"), None, "{}", output.out);
    assert_eq!(value(&output.out, "writes"), None);

    status::write(&setup.status_path(), &own).expect("the status file");
    let output = setup.status();
    assert_eq!(value(&output.out, "reads"), Some("5"));
}

/// `create` takes the write unit from the block size of the
/// directory's file system and refuses, in plain words, a size the
/// format does not carry and one that cannot be read.
#[test]
fn create_chooses_the_write_unit_by_the_file_systems_block_or_refuses() {
    let system = Identity::system_write_unit();
    assert_eq!(write_unit_on(Ok(4096)), Ok(system));
    assert_eq!(write_unit_on(Ok(65536)), Ok(65536));
    let large = write_unit_on(Ok(1 << 20)).expect_err("blocks of 1 MiB");
    assert!(
        large.contains("blocks of 1048576 bytes") && large.contains("at most 65536"),
        "{large}"
    );
    let odd = write_unit_on(Ok(12288)).expect_err("blocks of 12288");
    assert!(odd.contains("blocks of 12288 bytes"), "{odd}");
    let unread = write_unit_on(Err(io::ErrorKind::Unsupported.into())).expect_err("unreadable");
    assert!(unread.contains("could not be read"), "{unread}");
    // A unit that is not the page size is said; the usual one is not.
    assert_eq!(write_unit_note(4096, 4096), None);
    let note = write_unit_note(16384, 4096).expect("a note");
    assert!(
        note.contains("write unit is 16384 bytes") && note.contains("pages are 4096 bytes"),
        "{note}"
    );
}
