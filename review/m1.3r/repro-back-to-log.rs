
// Reproducer (review, second look at d1641a4): appended to
// crates/skyzfs-local/src/inplace/tests.rs, run, and taken out again.

/// Read one block through the log store.
fn log_read(store: &mut Store<SimDisk>, block: u64) -> Vec<u8> {
    store
        .submit(Request {
            id: RequestId(500 + block),
            op: Op::Read {
                offset: block * BLOCK,
                buf: Buf::filled(1, 0xEE),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, Duration::ZERO);
    }
    assert_eq!(done[0].result, Ok(()));
    done[0].buf.as_deref().unwrap().to_vec()
}

/// The in-place store acknowledges a write and is lost (no power cut).
/// The volume is started with the log store, which acknowledges a
/// write elsewhere and a flush. Power is cut. `back_in_place` starts
/// the in-place store instead, as a control.
fn repro_back(back_in_place: bool) -> (u8, u8) {
    let disk = created(6, 0);
    let mut store = start(&disk, 0, false).unwrap();
    write(&mut store, 7, 0x77).unwrap();
    drop(store);
    disk.kill();
    if back_in_place {
        let mut store = start(&disk, 0, false).unwrap();
        write(&mut store, 9, 0x99).unwrap();
        assert_eq!(run(&mut store, 2, Op::Flush).result, Ok(()));
        drop(store);
    } else {
        let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
        assert_eq!(log_read(&mut store, 7), vec![0x77; BLOCK_USIZE], "before the cut");
        let mut done = Vec::new();
        store
            .submit(Request {
                id: RequestId(1),
                op: Op::Write {
                    offset: 9 * BLOCK,
                    data: Buf::filled(1, 0x99),
                },
            })
            .unwrap();
        store
            .submit(Request {
                id: RequestId(2),
                op: Op::Flush,
            })
            .unwrap();
        while done.len() < 2 {
            store.wait(&mut done, Duration::ZERO);
        }
        assert!(done.iter().all(|c| c.result.is_ok()), "{done:?}");
        drop(store);
    }
    disk.power_cut(crate::sim::LoseAll);
    let (mut store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
    (log_read(&mut store, 7)[0], log_read(&mut store, 9)[0])
}

#[test]
fn repro_control_back_in_place_the_flush_covers_the_earlier_write() {
    assert_eq!(repro_back(true), (0x77, 0x99));
}

#[test]
fn repro_back_to_the_log_store_the_flush_covers_the_earlier_write() {
    assert_eq!(repro_back(false), (0x77, 0x99));
}
