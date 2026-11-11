use super::*;
use skyzfs_blk::{Base, Buf, Completion, Op, Request, RequestId};
use skyzfs_local::format::Identity;
use skyzfs_local::store::StoreConfig;
use std::io;
use std::time::Duration;

fn created(size_mib: u64) -> (tempfile::TempDir, PathBuf, [u8; 16], u64) {
    let keep = tempfile::tempdir().expect("a directory");
    let dir = keep.path().join("volume");
    std::fs::create_dir(&dir).expect("the volume's directory");
    let config = StoreConfig {
        volume: *b"a volume on disk",
        size_mib,
        base: Base::Empty,
        align: 4096,
        segment_size: 8 << 20,
        write_unit: Identity::system_write_unit(),
    };
    let mut store = Store::create(
        FsDir::new(&dir).expect("the directory"),
        &config,
        OpenOptions::default(),
    )
    .expect("create");
    let identity = store.identity();
    // One write, so that the log has a record to reach.
    store
        .submit(Request {
            id: RequestId(1),
            op: Op::Write {
                offset: 0,
                data: Buf::filled(1, 7),
            },
        })
        .expect("submit");
    let mut done: Vec<Completion> = Vec::new();
    assert_eq!(store.wait(&mut done, Duration::from_secs(10)), 1);
    assert_eq!(done[0].result, Ok(()));
    let end = store.position().0;
    drop(store);
    (keep, dir, identity, end)
}

#[test]
fn the_lock_is_a_step_of_its_own_and_the_open_takes_it_over() {
    let (_keep, dir, identity, end) = created(8);
    let mut volume = StoreVolume::new(&dir, OpenOptions::default());
    let mut other = volume.clone();

    let lock = volume.lock().expect("free");
    assert_eq!(other.lock().err(), Some(LockError::Locked));

    // The log does not reach its own end: refused, and the lock comes
    // back still held.
    let beyond = MustReach {
        identity,
        position: end,
    };
    let lock = match volume.open(lock, Some(beyond)) {
        Err((OpenError::Short, lock)) => lock,
        other => panic!("{:?}", other.map(|(_, size)| size).map_err(|(e, _)| e)),
    };
    assert_eq!(other.lock().err(), Some(LockError::Locked));

    // A position of another store counts as none.
    let anothers = MustReach {
        identity: [9; 16],
        position: end,
    };
    let (store, size) = match volume.open(lock, Some(anothers)) {
        Ok(opened) => opened,
        Err((e, _)) => panic!("{e:?}"),
    };
    assert_eq!(size, 8 << 20);
    assert_eq!(store.identity(), identity);
    assert_eq!(
        other.lock().err(),
        Some(LockError::Locked),
        "the store holds it"
    );
    drop(store);
    drop(other.lock().expect("free again"));

    // A position the log reaches passes.
    let lock = volume.lock().expect("free");
    let reached = MustReach {
        identity,
        position: end - 1,
    };
    assert!(volume.open(lock, Some(reached)).is_ok());
}

#[test]
fn a_directory_that_is_not_a_volume_is_an_error_that_deletes_nothing() {
    let keep = tempfile::tempdir().expect("a directory");
    let mut volume = StoreVolume::new(&keep.path().join("missing"), OpenOptions::default());
    assert!(matches!(volume.lock(), Err(LockError::Other(_))));

    // A directory with no log in it: the lock can be had, the open is
    // the kind of error that may pass.
    let mut volume = StoreVolume::new(keep.path(), OpenOptions::default());
    let lock = volume.lock().expect("free");
    assert!(matches!(
        volume.open(lock, None),
        Err((OpenError::Other(_), _))
    ));
}

#[test]
fn the_stores_errors_are_sorted_into_the_four_kinds() {
    let io = |failed| {
        StoreError::Log(LogError::Io {
            during: "syncing before recovery",
            kind: io::ErrorKind::Other,
            marker_written: false,
            failed,
        })
    };
    let short = StoreError::Log(LogError::Short {
        end: Lsn(10),
        must_reach: Lsn(20),
    });
    assert_eq!(classify(&short), OpenError::Short);
    assert_eq!(
        classify(&StoreError::Log(LogError::MarkedFailed)),
        OpenError::MarkedFailed
    );
    // A sync or a write that failed in the recovery, whether or not the
    // marker could be written, and a log that had failed already.
    assert_eq!(classify(&io(true)), OpenError::SyncFailed);
    assert_eq!(
        classify(&StoreError::Log(LogError::Failed)),
        OpenError::SyncFailed
    );
    // A read that failed may pass.
    assert!(matches!(classify(&io(false)), OpenError::Other(_)));
    for passing in [
        StoreError::Log(LogError::NotFound),
        StoreError::Log(LogError::Corrupt(Corruption::NoSuperblock)),
        StoreError::Damaged("there is no home image"),
        StoreError::Locked,
    ] {
        assert!(
            matches!(classify(&passing), OpenError::Other(_)),
            "{passing:?}"
        );
    }
}
