use super::*;
use crate::io::{Dir, File};
use crate::sim::{LoseAll, SeededCut, SimDisk};

fn sample() -> Volume {
    Volume {
        id: [7; 16],
        size: 1 << 20,
        block_size: 4096,
        write_unit: 4096,
        identity: [9; 16],
    }
}

/// What is created reads back, and names the volume by its identity.
#[test]
fn a_volume_file_reads_back_as_it_was_created() {
    let disk = SimDisk::new(1);
    sample().create(&disk).unwrap();
    assert_eq!(Volume::read(&disk), Ok(sample()));
    assert_eq!(identity_of(&disk), Some([9; 16]));
    let names = disk.list().unwrap();
    assert!(names.iter().any(|n| n == VOLUME_NAME));
    assert!(!names.iter().any(|n| n == VOLUME_NEW_NAME), "{names:?}");
}

/// The layout is the documented one.
#[test]
fn the_file_has_the_documented_layout() {
    let bytes = sample().encode();
    let mut expected = Vec::new();
    expected.extend(b"SKYZVOL1");
    expected.extend([7; 16]);
    expected.extend((1u64 << 20).to_le_bytes());
    expected.extend(4096u32.to_le_bytes());
    expected.extend(4096u32.to_le_bytes());
    expected.extend([9; 16]);
    expected.extend(crc32c::crc32c(&expected).to_le_bytes());
    assert_eq!(bytes.to_vec(), expected);
    assert_eq!(bytes.len(), 60);
}

/// A create that returned is durable: a power cut right after it, which
/// keeps nothing that was not synced, leaves the file whole, under its
/// name.
#[test]
fn a_create_that_returned_survives_a_power_cut() {
    let disk = SimDisk::new(2);
    sample().create(&disk).unwrap();
    disk.power_cut(LoseAll);
    assert_eq!(Volume::read(&disk), Ok(sample()));
}

/// A cut inside the create leaves either the whole file or no file
/// under its name, never a part of one: the power goes after each of
/// the create's writes and syncs in turn, and whatever a cut keeps of
/// what was not synced, a read finds the volume or finds none.
#[test]
fn a_cut_inside_a_create_leaves_the_whole_file_or_none() {
    let mut found = [0u32; 2];
    for calls in 0..4 {
        for seed in 0..64 {
            let disk = SimDisk::new(seed);
            disk.halt_after(calls);
            let created = sample().create(&disk);
            disk.power_cut(SeededCut::new(seed));
            match Volume::read(&disk) {
                Ok(volume) => {
                    assert_eq!(volume, sample(), "after {calls} calls, seed {seed}");
                    found[0] += 1;
                }
                Err(VolumeError::Missing) => {
                    assert!(created.is_err(), "after {calls} calls, seed {seed}: lost");
                    found[1] += 1;
                }
                Err(other) => panic!("after {calls} calls, seed {seed}: {other}"),
            }
        }
    }
    // Both outcomes happen, so the cuts reach inside the create.
    assert!(found[0] > 0 && found[1] > 0, "{found:?}");
}

/// No file is a directory without a volume; the log store's superblock
/// without one is a volume of a daemon from before M1.3t.
#[test]
fn no_file_is_missing_and_a_log_superblock_is_legacy() {
    let disk = SimDisk::new(3);
    assert_eq!(Volume::read(&disk), Err(VolumeError::Missing));
    assert_eq!(identity_of(&disk), None);
    disk.create(LEGACY_NAME).unwrap();
    assert_eq!(Volume::read(&disk), Err(VolumeError::Legacy));
    let said = VolumeError::Legacy.to_string();
    assert!(
        said.contains("log store") && said.contains("another directory"),
        "{said}"
    );
}

/// A flipped byte anywhere is damage; another length or magic is a
/// version this daemon does not know; values no volume has are damage.
#[test]
fn a_damaged_or_unknown_file_is_refused_by_reason() {
    let write = |bytes: &[u8]| {
        let disk = SimDisk::new(4);
        let mut file = disk.create(VOLUME_NAME).unwrap();
        file.write_at(0, bytes).unwrap();
        file.sync().unwrap();
        Volume::read(&disk)
    };
    let good = sample().encode();
    for at in 0..good.len() {
        let mut bytes = good;
        bytes[at] ^= 0x10;
        let read = write(&bytes);
        if at < 8 {
            assert!(
                matches!(read, Err(VolumeError::Unknown { .. })),
                "byte {at}: {read:?}"
            );
        } else {
            assert_eq!(
                read,
                Err(VolumeError::Damaged("its checksum does not match")),
                "byte {at}"
            );
        }
    }
    let read = write(&good[..40]);
    assert!(
        matches!(read, Err(VolumeError::Unknown { len: 40, .. })),
        "{read:?}"
    );
    let mut longer = good.to_vec();
    longer.extend([0; 8]);
    assert!(matches!(
        write(&longer),
        Err(VolumeError::Unknown { len: 68, .. })
    ));
    let mut later = good;
    later[..8].copy_from_slice(b"SKYZVOL2");
    let said = write(&later).unwrap_err().to_string();
    assert!(
        said.contains("SKYZVOL2") && said.contains("newer daemon"),
        "{said}"
    );
    let odd = Volume {
        write_unit: 3000,
        ..sample()
    };
    assert!(matches!(write(&odd.encode()), Err(VolumeError::Damaged(_))));
    // A block size other than the contract's is not served in blocks
    // of 4096 bytes: the file is refused.
    for block_size in [512, 8192] {
        let other = Volume {
            block_size,
            ..sample()
        };
        assert_eq!(
            write(&other.encode()),
            Err(VolumeError::Damaged(
                "the block size is not the one this version has"
            )),
            "{block_size}"
        );
    }
}

/// A second create is refused and leaves the first; a file left under
/// the other name by a create that did not finish is replaced.
#[test]
fn a_second_create_is_refused_and_a_leftover_is_replaced() {
    let disk = SimDisk::new(5);
    let mut left = disk.create(VOLUME_NEW_NAME).unwrap();
    left.write_at(0, b"half a file").unwrap();
    drop(left);
    sample().create(&disk).unwrap();
    assert_eq!(Volume::read(&disk), Ok(sample()));
    let other = Volume {
        identity: [1; 16],
        ..sample()
    };
    let refused = other.create(&disk).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(Volume::read(&disk), Ok(sample()));
}

/// The write unit of a new volume is the page or the file system's
/// block, whichever is larger, within what a volume can have.
#[test]
fn the_write_unit_is_the_larger_of_page_and_block() {
    let page = system_write_unit();
    assert!((MIN_WRITE_UNIT..=MAX_WRITE_UNIT).contains(&page));
    assert_eq!(write_unit_for(512), Some(page));
    assert_eq!(write_unit_for(65536), Some(65536));
    assert_eq!(write_unit_for(131_072), None);
    assert_eq!(write_unit_for(3000), None);
}
