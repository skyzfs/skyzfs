
// Review experiment for #55 at 754a15a (not part of the product). Append
// to crates/skyzfs-local/src/inplace/tests.rs in a worktree and run
// `cargo test -p skyzfs-local --lib zz_review -- --nocapture`.
//
// What a failed start of the in-place store is reported as, for a log
// that holds nothing, one write, or one discard: at each sync the start
// makes, and for each file whose every sync fails.

fn zz_disk(kind: u8) -> SimDisk {
    let disk = SimDisk::new(12);
    disk.set_write_unit(4096);
    disk.set_page_size(4096);
    disk.set_block_size(Some(4096));
    disk.set_sync_delay(0);
    let config = StoreConfig {
        volume: *b"in-place volume.",
        size_mib: 1,
        base: Base::Empty,
        align: 64,
        segment_size: (1 << 20) + (128 << 10),
        write_unit: 4096,
    };
    let mut store = Store::create_keyed(
        disk.clone(),
        &config,
        Key(*b"a key for tests!"),
        OpenOptions::default(),
    )
    .unwrap();
    let op = match kind {
        0 => None,
        1 | 3 => Some(Op::Write {
            offset: 3 * BLOCK,
            data: Buf::filled(1, 0x33),
        }),
        _ => Some(Op::Discard {
            offset: 3 * BLOCK,
            len: BLOCK,
        }),
    };
    if let Some(op) = op {
        store
            .submit(Request {
                id: RequestId(0),
                op,
            })
            .unwrap();
        let mut done = Vec::new();
        store.wait(&mut done, Duration::ZERO);
        assert_eq!(done[0].result, Ok(()));
    }
    drop(store);
    disk.kill();
    if kind == 3 {
        // As the pull request's new test: a first start applies the log.
        drop(start(&disk, 0, false).unwrap());
        disk.kill();
    }
    disk
}

fn zz_said(disk: &SimDisk, result: Result<InPlace<SimDisk>, IntoInPlaceError>) -> (String, bool) {
    match result {
        Ok(_) => ("started".to_string(), true),
        Err(IntoInPlaceError::Failed(failed)) => {
            let marked = disk.list().unwrap().iter().any(|n| n == MARKER_NAME);
            (
                format!(
                    "{:?} (marker {} the hand-back)",
                    failed.failed(),
                    if marked { "written before" } else { "left to" }
                ),
                false,
            )
        }
        Err(other) => (format!("other: {other}"), true),
    }
}

#[test]
fn zz_review_what_each_start_says() {
    for (label, kind) in [
        ("a log that never held a record", 0u8),
        ("a log emptied by an earlier start in place", 3),
        ("a log with one write", 1),
        ("a log with one discard and no write", 2),
    ] {
        let files = zz_disk(kind).list().unwrap();
        {
            let disk = zz_disk(kind);
            let (store, opened) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
            drop(store);
            println!("== {label}: files {files:?}; the open says {opened:?}");
        }
        for nth in 0.. {
            let disk = zz_disk(kind);
            let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
            disk.fail_sync(nth, false);
            let (said, last) = zz_said(&disk, store.into_in_place(None));
            println!("   sync {nth} fails: {said}");
            if last {
                break;
            }
        }
        for name in &files {
            let disk = zz_disk(kind);
            let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
            disk.fail_sync_of(name);
            let (said, _) = zz_said(&disk, store.into_in_place(None));
            println!("   every sync of {name} fails: {said}");
        }
    }
}
