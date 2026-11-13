//! Review experiment for #55 (not part of the product): a failure note
//! whose reason code the reading daemon does not know, and what a start
//! does once someone has removed that note by hand, as the format
//! note in `records.rs` advises for a daemon older than code 7.
//!
//! This version's unknown code is 8; it stands for what 7 is to the
//! older daemon (the records' own test says the two are refused alike).
//!
//! Copy to crates/skyzfs-ublk/tests/ and run
//! `cargo test -p skyzfs-ublk --test zz_review_removed_note -- --nocapture`.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use skyzfs_blk::Base;
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::store::{OpenOptions, Store, StoreConfig};
use skyzfs_ublk::lifecycle::{
    fail_serving, start, AddError, Control, DeviceInfo, DeviceState, Reason, Shape, Stop, Wanted,
};
use skyzfs_ublk::records::{read_failure, Files};
use skyzfs_ublk::volume::StoreVolume;

const BOOT: [u8; 16] = [7; 16];

#[derive(Default)]
struct Kernel {
    devices: BTreeMap<u32, DeviceInfo>,
    held: Vec<u32>,
}

impl Control for Kernel {
    fn features(&mut self) -> io::Result<Option<u64>> {
        Ok(Some(u64::MAX))
    }
    fn release(&mut self) -> io::Result<String> {
        Ok("7.0.0".to_string())
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        Ok(self.devices.get(&number).copied())
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        if self.devices.contains_key(&number) {
            return Err(AddError::Taken);
        }
        self.devices.insert(
            number,
            DeviceInfo {
                state: DeviceState::Dead,
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
        async_delete_asserted: false,
    }
}

fn said(result: &Result<impl Sized, Stop>) -> String {
    match result {
        Err(Stop::Failed(why)) => format!("REFUSED as failed: {why}"),
        Err(other) => format!("stopped otherwise: {other:?}"),
        Ok(_) => "SERVED".to_string(),
    }
}

fn note(records: &Files) -> String {
    match read_failure(&records.failure_path()) {
        Ok(None) => "no note".to_string(),
        Ok(Some(n)) => format!("note: {:?}, held {:?}", n.failure.reason, n.failure.held),
        Err(e) => format!("note unreadable: {e}"),
    }
}

/// `held`: a client holds the device at the failure (a suspended pool).
fn scenario(held: bool) {
    println!("\n=== a client {} the device at the failure ===", if held { "HOLDS" } else { "does not hold" });
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
    drop(
        Store::create(
            FsDir::new(&dir).expect("the directory"),
            &config,
            OpenOptions::default(),
        )
        .expect("create"),
    );
    let mut kernel = Kernel::default();
    let mut records = Files::new(&dir as &Path, &run, BOOT, Duration::ZERO).expect("records");
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());

    let ready = start(&mut kernel, &mut records, &mut volume, &wanted(false)).expect("serves");
    let (number, identity) = (ready.number, ready.store.identity());
    if let Some(device) = kernel.devices.get_mut(&number) {
        device.state = DeviceState::Live;
    }
    if held {
        kernel.held.push(number);
    }
    // The volume fails while it is served; the store writes its marker.
    let served = ready.store.serve().expect("serve");
    served.fail_now();
    let (failure, written, _) = fail_serving(
        &mut kernel,
        &mut records,
        number,
        identity,
        Reason::ImageFailed,
    );
    drop(served);
    println!("the failure: {failure:?}, written {written}; {}", note(&records));

    // The reading daemon does not know the code.
    let path = records.failure_path();
    let mut bytes = std::fs::read(&path).expect("the note");
    assert_eq!(bytes[41], 7);
    bytes[41] = 8;
    std::fs::write(&path, bytes).expect("write");
    for acknowledge in [false, true] {
        let result = start(&mut kernel, &mut records, &mut volume, &wanted(acknowledge));
        println!("unknown code, acknowledge {acknowledge}: {}", said(&result));
    }

    // Someone removes the note by hand (the old advice).
    std::fs::remove_file(&path).expect("remove");
    for acknowledge in [false, true] {
        let result = start(&mut kernel, &mut records, &mut volume, &wanted(acknowledge));
        println!(
            "note removed by hand, acknowledge {acknowledge}: {}; then {}",
            said(&result),
            note(&records)
        );
        if result.is_ok() {
            break;
        }
    }

    // The corrected advice: the note AND the store's marker.
    let marker = dir.join("log.failed");
    println!("the marker is there: {}", marker.exists());
    let _ = std::fs::remove_file(&records.failure_path());
    std::fs::remove_file(&marker).expect("remove the marker");
    for acknowledge in [false, true] {
        let result = start(&mut kernel, &mut records, &mut volume, &wanted(acknowledge));
        println!(
            "note and marker removed, acknowledge {acknowledge}: {}; then {}",
            said(&result),
            note(&records)
        );
        if result.is_ok() {
            break;
        }
    }

    // And what a reboot does with the marker alone, as the note now says.
    let mut next = Files::new(&dir as &Path, &run, [9; 16], Duration::ZERO).expect("records");
    let result = start(&mut kernel, &mut next, &mut volume, &wanted(false));
    println!("after a reboot, no flag: {}", said(&result));
}

/// The other half of the corrected paragraph: the note is of this boot
/// only, so after a reboot the marker alone is left, and the start
/// acknowledges it with --acknowledge-failure.
fn reboot_with_the_marker(held: bool) {
    println!("\n=== after a reboot, the marker alone (a client {} the device) ===",
             if held { "held" } else { "did not hold" });
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
    drop(Store::create(FsDir::new(&dir).expect("the directory"), &config, OpenOptions::default())
        .expect("create"));
    let mut kernel = Kernel::default();
    let mut records = Files::new(&dir as &Path, &run, BOOT, Duration::ZERO).expect("records");
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());
    let ready = start(&mut kernel, &mut records, &mut volume, &wanted(false)).expect("serves");
    let (number, identity) = (ready.number, ready.store.identity());
    if let Some(device) = kernel.devices.get_mut(&number) {
        device.state = DeviceState::Live;
    }
    if held {
        kernel.held.push(number);
    }
    let served = ready.store.serve().expect("serve");
    served.fail_now();
    let _ = fail_serving(&mut kernel, &mut records, number, identity, Reason::ImageFailed);
    drop(served);
    println!("the marker is there: {}", dir.join("log.failed").exists());
    // The next boot: the note counts as none, and the record of the
    // attachment is of the earlier boot.
    let mut next = Files::new(&dir as &Path, &run, [9; 16], Duration::ZERO).expect("records");
    for acknowledge in [false, true] {
        let result = start(&mut kernel, &mut next, &mut volume, &wanted(acknowledge));
        println!("after a reboot, acknowledge {acknowledge}: {}", said(&result));
        if let Ok(ready) = result {
            println!("    acknowledged: {:?}", ready.acknowledged);
            break;
        }
    }
}

#[test]
fn what_removing_the_note_by_hand_leads_to() {
    scenario(false);
    scenario(true);
    reboot_with_the_marker(false);
    reboot_with_the_marker(true);
}
