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
}

impl Control for Kernel {
    fn can_reissue(&mut self) -> io::Result<bool> {
        Ok(true)
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
        detach(&self.volume, &mut self.kernel, BOOT)
    }
}

fn value<'a>(out: &'a str, key: &str) -> Option<&'a str> {
    out.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(' '))
}

fn sample() -> Status {
    Status {
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
        detach(&volume, &mut kernel, BOOT),
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
    status::write(&setup.status_path(), &sample()).expect("the status file");
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
        ("pid", "4242"),
        ("age_ms", "250"),
    ] {
        assert_eq!(value(&output.out, key), Some(wanted), "{key}");
    }

    // The server is gone: no pid to kill, and the counters are said to
    // be left behind.
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    status::write(&setup.status_path(), &sample()).expect("the status file");
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

    // Deleted and still held: 0, and said on standard error.
    let mut setup = Setup::new().attached(DeviceState::Waiting);
    setup.kernel.held.push(4);
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Done);
    assert!(output.out.is_empty());
    assert!(output.err.contains("still holds it open"), "{}", output.err);
    assert_eq!(setup.kernel.deleted, [4]);

    // Deleted before and still held looks like never started: it is
    // deleted again, which does no harm, and said to be held.
    let mut setup = Setup::new().attached(DeviceState::Dead);
    setup.kernel.held.push(4);
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Done);
    assert!(output.err.contains("still holds it open"), "{}", output.err);
    assert_eq!(setup.kernel.deleted, [4]);

    // No server, no note, the recorded values: deleted before and
    // still held, or another's. 0, said, nothing done, the record stays.
    let mut setup = Setup::new().attached(DeviceState::Dead);
    setup.records().remove_note(4).expect("the note");
    let output = setup.detach();
    assert_eq!(output.exit, Exit::Done);
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
