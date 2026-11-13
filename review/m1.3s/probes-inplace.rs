
/// Review probe: how often the suite notices a flush that joins any sync.
#[test]
fn probe_j_suite_vs_flush_joins_any_sync() {
    for i in 0..3 {
        let mut noticed = 0;
        for seed in 0..8u64 {
            let config = Config {
                cases: 32,
                seed,
                ..Config::default()
            };
            let mut harness = harnesses(Mutants::FLUSH_JOINS_ANY_SYNC)[i];
            let report = run_all(&mut harness, &config);
            if !report.passed() {
                noticed += 1;
                eprintln!("probe J: harness {i} seed {seed}: {:?}", report.failed());
            }
        }
        eprintln!("probe J: harness {i}: noticed at {noticed} of 8 seeds");
    }
}

/// Review probe: the suite on the in-place store at other seeds, with
/// no defect planted: does the model refuse anything the store does?
#[test]
fn probe_k_other_seeds() {
    for seed in 1..=6u64 {
        let config = Config {
            cases: 32,
            seed,
            ..Config::default()
        };
        for (i, mut harness) in harnesses(0).into_iter().enumerate() {
            let report = run_all(&mut harness, &config);
            eprintln!(
                "probe K: seed {seed} harness {i}: passed {} failed {:?}",
                report.passed(),
                report.failed()
            );
            if !report.passed() {
                eprintln!("{report}");
            }
        }
    }
}

/// Review probe: which cases notice a flush that syncs nothing, with
/// the harness as it is.
#[test]
fn probe_l_which_cases_notice() {
    for mutant in [
        Mutants::FLUSH_WITHOUT_SYNC,
        Mutants::SYNC_RETRIED,
        Mutants::WRITE_ERROR_PASSES,
    ] {
        for (i, mut harness) in harnesses(mutant).into_iter().enumerate() {
            let config = Config {
                cases: 32,
                ..Config::default()
            };
            let report = run_all(&mut harness, &config);
            eprintln!("probe L: mutant {mutant:#x} harness {i}: {:?}", report.failed());
        }
    }
}

/// Review probe: a start whose conversion meets a failed sync, at each
/// of the syncs it makes: what it returns and what the next open finds.
#[test]
fn probe_m_a_start_that_meets_a_failed_sync() {
    for sticky in [false, true] {
        for n in 0..8u64 {
            let disk = created(11, 0);
            let (store, _) = Store::open(disk.clone(), OpenOptions::default()).unwrap();
            disk.fail_sync(n, sticky);
            let got = store.into_in_place_with(None, Mutants(0));
            let said = got.as_ref().err().cloned();
            drop(got);
            disk.clear_faults();
            disk.kill();
            let again = Store::open(disk.clone(), OpenOptions::default());
            eprintln!(
                "probe M: sticky {sticky} sync {n}: conversion {said:?}; next open: {:?}",
                again.as_ref().err().map(|e| e.to_string())
            );
        }
    }
}

/// Review probe: a failed volume reopened in the boot in which it
/// failed (the process is lost, the cache is kept), then a write and a
/// flush that completes, then a power cut that keeps nothing unsynced.
#[test]
fn probe_n_a_reopen_without_a_cut_and_a_flush_after_it() {
    let mut lost = 0;
    for seed in 0..64u64 {
        let disk = created(seed, 0);
        let mut store = start(&disk, 0, false).unwrap();
        write(&mut store, 1, 0x11).unwrap();
        assert_eq!(run(&mut store, 1, Op::Flush).result, Ok(()));
        // Acknowledged, not flushed.
        write(&mut store, 4, 0x44).unwrap();
        disk.fail_sync(0, false);
        assert_eq!(run(&mut store, 2, Op::Flush).result, Err(BlkError::Failed));
        drop(store);
        disk.clear_faults();
        disk.kill();
        let mut store = start(&disk, 0, true).unwrap();
        let found = read(&mut store, 4).unwrap();
        // A healthy volume again: a write and a flush that completes.
        write(&mut store, 2, 0x22).unwrap();
        assert_eq!(run(&mut store, 3, Op::Flush).result, Ok(()));
        drop(store);
        disk.power_cut(crate::sim::LoseAll);
        let mut store = start(&disk, 0, false).unwrap();
        let after = read(&mut store, 4).unwrap();
        if after != found {
            lost += 1;
            eprintln!(
                "probe N: seed {seed}: block 4 read as {:#x} after the reopen, and as {:#x} after a completed flush and a power cut",
                found[0], after[0]
            );
        }
    }
    eprintln!("probe N: at {lost} of 64 seeds the reopened volume lost what its start showed");
}
