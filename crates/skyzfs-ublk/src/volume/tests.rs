use super::*;
use skyzfs_blk::Base;
use skyzfs_local::inplace::VolumeConfig;
use skyzfs_local::volume::{system_write_unit, VolumeError, MARKER_NAME};

fn created(size_mib: u64) -> (tempfile::TempDir, PathBuf) {
    let keep = tempfile::tempdir().expect("a directory");
    let dir = keep.path().join("volume");
    std::fs::create_dir(&dir).expect("the volume's directory");
    let config = VolumeConfig {
        volume: *b"a volume on disk",
        size_mib,
        base: Base::Empty,
        write_unit: system_write_unit(),
    };
    inplace::create(&FsDir::new(&dir).expect("the directory"), &config).expect("create");
    (keep, dir)
}

#[test]
fn the_lock_is_a_step_of_its_own_and_the_open_takes_it_over() {
    let (_keep, dir) = created(8);
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());
    let mut other = volume.clone();

    let lock = volume.lock().expect("free");
    assert_eq!(other.lock().err(), Some(LockError::Locked));

    let (opened, size) = match volume.open(lock, false) {
        Ok(opened) => opened,
        Err((e, _)) => panic!("{e:?}"),
    };
    assert_eq!(size, 8 << 20);
    let identity = skyzfs_local::inplace::identity_of(&FsDir::new(&dir).unwrap());
    assert_eq!(Some(opened.identity()), identity);
    assert_eq!(
        other.lock().err(),
        Some(LockError::Locked),
        "the opened volume holds it"
    );
    drop(opened);
    drop(other.lock().expect("free again"));
}

#[test]
fn a_directory_that_is_not_a_volume_is_an_error_that_deletes_nothing() {
    let keep = tempfile::tempdir().expect("a directory");
    let mut volume = StoreVolume::new(&keep.path().join("missing"), OpenOptions::default());
    assert!(matches!(volume.lock(), Err(LockError::Other(_))));

    // A directory with no volume file in it: the lock can be had, the
    // open is the kind of error that may pass.
    let mut volume = StoreVolume::new(keep.path(), OpenOptions::default());
    let lock = volume.lock().expect("free");
    assert!(matches!(
        volume.open(lock, false),
        Err((OpenError::Other(_), _))
    ));
}

/// The failure marker refuses the open as `MarkedFailed`, the refusal
/// hands the lock back still held, and an open that acknowledges the
/// marker removes it. (The held lock stands in for part of the log
/// store's `the_lock_is_a_step_of_its_own_and_the_open_takes_it_over`.)
#[test]
fn a_marked_volume_is_refused_until_acknowledged() {
    let (_keep, dir) = created(8);
    std::fs::write(dir.join(MARKER_NAME), b"").unwrap();
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());
    let lock = volume.lock().expect("free");
    let lock = match volume.open(lock, false) {
        Err((OpenError::MarkedFailed, lock)) => lock,
        other => panic!("{:?}", other.map(|(_, size)| size).map_err(|(e, _)| e)),
    };
    assert!(
        dir.join(MARKER_NAME).exists(),
        "an open that refuses writes nothing"
    );
    // The refusal hands the lock back still held: nothing else takes
    // the volume between the refusal and the acknowledged open.
    assert_eq!(
        StoreVolume::new(&dir, OpenOptions::default()).lock().err(),
        Some(LockError::Locked)
    );
    let (opened, _) = volume
        .open(lock, true)
        .map_err(|(e, _)| e)
        .expect("acknowledged");
    assert!(opened.acknowledged());
    assert!(!dir.join(MARKER_NAME).exists());
}

#[test]
fn the_stores_errors_are_sorted_into_their_kinds() {
    use skyzfs_local::inplace::OpenError as E;
    assert_eq!(classify(&E::MarkedFailed), OpenError::MarkedFailed);
    // A volume this daemon does not serve here: a refusal with its
    // reason, and no failure of the volume.
    let blocks = |what| E::Blocks {
        what,
        block: 16384,
        unit: 4096,
    };
    use skyzfs_local::inplace::Under;
    for (refused, says) in [
        (blocks(Under::Directory), "blocks of 16384 bytes"),
        // The home image on a file system of its own (M1.3ae).
        (
            blocks(Under::Home),
            "home image (home.img) is on a file system with blocks of 16384 bytes",
        ),
        (
            E::BlocksUnknown {
                what: Under::Home,
                kind: std::io::ErrorKind::Unsupported,
            },
            "the file system under the volume's home image (home.img) could not be read",
        ),
        (E::Volume(VolumeError::Legacy), "log store"),
        (E::Volume(VolumeError::Missing), "no volume file"),
        (
            E::Volume(VolumeError::Damaged("its checksum does not match")),
            "checksum",
        ),
        (E::Damaged("there is no home image"), "no home image"),
    ] {
        match classify(&refused) {
            OpenError::Other(why) => assert!(
                why.contains(says)
                    && why.contains("the volume is not failed")
                    && why.contains("is left as it is; `skyzfsd detach` removes it"),
                "{why}"
            ),
            other => panic!("{other:?}"),
        }
    }
    for passing in [
        E::Locked,
        E::Io {
            during: "removing the failure marker",
            kind: std::io::ErrorKind::Other,
        },
    ] {
        assert!(
            matches!(classify(&passing), OpenError::Other(_)),
            "{passing:?}"
        );
    }
}
