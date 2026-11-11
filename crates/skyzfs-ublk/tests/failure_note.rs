//! The failure note over the real store and the real files, with a
//! kernel of the test's own: a log that is marked as failed, the
//! operator's acknowledgement, and a failure while a client may hold
//! the device.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use skyzfs_blk::Base;
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::store::{OpenOptions, Store, StoreConfig};
use skyzfs_ublk::lifecycle::{
    fail_serving, start, AddError, Control, DeviceInfo, DeviceState, Held, Reason, Shape, Stop,
    Wanted,
};
use skyzfs_ublk::records::{read_failure, Files};
use skyzfs_ublk::volume::StoreVolume;

const BOOT: [u8; 16] = [7; 16];

/// A kernel that keeps what it is told. A device in `held` has a
/// client that the kernel cannot report: deleted, it keeps its number.
#[derive(Default)]
struct Kernel {
    devices: BTreeMap<u32, DeviceInfo>,
    held: Vec<u32>,
}

impl Control for Kernel {
    fn can_reissue(&mut self) -> io::Result<bool> {
        Ok(true)
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        Ok(self.devices.get(&number).copied())
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        if self.devices.contains_key(&number) {
            return Err(AddError::Taken);
        }
        let state = DeviceState::Dead;
        self.devices.insert(
            number,
            DeviceInfo {
                state,
                shape: *shape,
            },
        );
        Ok(())
    }
    fn disown(&mut self, _number: u32) {}
    fn delete(&mut self, number: u32) -> io::Result<()> {
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
        Ok(())
    }
    fn client_holds(&mut self, _number: u32) -> Option<bool> {
        None
    }
}

fn wanted(acknowledge_failure: bool) -> Wanted {
    Wanted {
        queues: 1,
        depth: 64,
        flags: 0,
        lock_retries: 0,
        live_retries: 0,
        gone_retries: 0,
        acknowledge_failure,
    }
}

fn files(volume: &Path, run: &Path, boot: [u8; 16]) -> Files {
    Files::new(volume, run, boot, Duration::ZERO).expect("the volume's records")
}

fn refused(result: Result<impl Sized, Stop>) -> String {
    match result {
        Err(Stop::Failed(why)) => why,
        Err(other) => panic!("not refused as failed: {other:?}"),
        Ok(_) => panic!("served"),
    }
}

#[test]
fn a_marked_volume_is_acknowledged_once_and_a_failure_while_serving_ends_the_boot() {
    let keep = tempfile::tempdir().expect("a directory");
    let (dir, run) = (keep.path().join("volume"), keep.path().join("run"));
    std::fs::create_dir(&dir).expect("the volume's directory");
    std::fs::create_dir(&run).expect("the run directory");
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
    // The log fails, with no daemon of this boot attached.
    let served = store.serve().expect("serve");
    served.fail_now();
    drop(served);

    let mut kernel = Kernel::default();
    let mut records = files(&dir, &run, BOOT);
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());

    // The marker, and no record of this boot: no client can hold a
    // device. Refused until it is acknowledged, and no note is written.
    let why = refused(start(
        &mut kernel,
        &mut records,
        &mut volume,
        &wanted(false),
    ));
    assert!(why.contains("--acknowledge-failure"), "{why}");
    assert_eq!(read_failure(&records.failure_path()).expect("read"), None);
    assert!(kernel.devices.is_empty());

    let ready = start(&mut kernel, &mut records, &mut volume, &wanted(true)).expect("serves");
    assert_eq!(ready.acknowledged, Some(Reason::Marked));
    let number = ready.number;
    let identity = ready.store.identity();
    if let Some(device) = kernel.devices.get_mut(&number) {
        device.state = DeviceState::Live;
    }

    // The log fails while the volume is served. A client holds the
    // device, and the kernel cannot say so: the note says unknown, and
    // the deleted device keeps its number.
    kernel.held.push(number);
    let served = ready.store.serve().expect("serve");
    served.fail_now();
    let (failure, written, _) = fail_serving(&mut kernel, &mut records, number, identity);
    assert!(failure.held == Held::Unknown && written);
    drop(served);
    assert_eq!(
        kernel.devices.get(&number).map(|d| d.state),
        Some(DeviceState::Dead)
    );
    let on_file = read_failure(&records.failure_path())
        .expect("read")
        .expect("a note");
    assert_eq!((on_file.boot, on_file.failure), (BOOT, failure));

    // The next start finds the dead device still under its number:
    // held. Not served again in this boot, whatever the operator says,
    // and the note on the file now says held.
    for acknowledge in [false, true] {
        let why = refused(start(
            &mut kernel,
            &mut records,
            &mut volume,
            &wanted(acknowledge),
        ));
        assert!(why.contains("not served again in this boot"), "{why}");
        let on_file = read_failure(&records.failure_path())
            .expect("read")
            .expect("a note");
        assert_eq!(on_file.failure.held, Held::Yes);
    }
    // Even after the client lets go: held stays held.
    kernel.held.clear();
    kernel.devices.clear();
    let why = refused(start(&mut kernel, &mut records, &mut volume, &wanted(true)));
    assert!(why.contains("not served again in this boot"), "{why}");

    // After a reboot the note and the record mean nothing: the marker
    // is what is left, and it is acknowledged.
    let mut records = files(&dir, &run, [8; 16]);
    let why = refused(start(
        &mut kernel,
        &mut records,
        &mut volume,
        &wanted(false),
    ));
    assert!(why.contains("--acknowledge-failure"), "{why}");
    let ready = start(&mut kernel, &mut records, &mut volume, &wanted(true)).expect("serves");
    assert_eq!(ready.acknowledged, Some(Reason::Marked));
    drop(ready);

    // And from then on it is a volume like any other.
    let ready = start(&mut kernel, &mut records, &mut volume, &wanted(false)).expect("serves");
    assert_eq!(ready.acknowledged, None);
}
