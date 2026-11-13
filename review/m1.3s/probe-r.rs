
// ---- review probe (not part of the pull request) ----

/// Probe R: a healthy volume whose block 0 was never written and whose
/// blocks PROBE_BLOCK.. were (a ZFS label without a partition table has
/// that shape: 8 KiB never written, then the boot block), started
/// PROBE_STARTS times in the directory PROBE_DIR names. How many starts
/// are refused, and why?
#[test]
fn probe_r_starts_of_a_volume_with_a_hole_at_its_first_block() {
    let Ok(path) = std::env::var("PROBE_DIR") else {
        return;
    };
    let starts: u32 = std::env::var("PROBE_STARTS").unwrap().parse().unwrap();
    let block: u64 = std::env::var("PROBE_BLOCK").unwrap().parse().unwrap();
    let path = std::path::PathBuf::from(path);
    let dir = on_disk(&path).expect("a directory whose cache can be dropped and counted");
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let mut store = start(&dir).unwrap();
    write(&mut store, block, Buf::filled(2, 0x22));
    flush(&mut store);
    drop(store);
    let mut refused = std::collections::BTreeMap::<String, u32>::new();
    let mut served = 0;
    for _ in 0..starts {
        let (store, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
        match store.into_in_place(None) {
            Ok(store) => {
                served += 1;
                drop(store);
            }
            Err(e) => *refused.entry(format!("{e}")).or_default() += 1,
        }
    }
    eprintln!("probe R: data from block {block}: {starts} starts, {served} served, refused: {refused:#?}");
}
