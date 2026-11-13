
// ---- review probe (not part of the pull request) ----

/// Probe O: one start of a new volume in the directory PROBE_DIR names,
/// whatever its file system (the tests' own helper would skip).
#[test]
fn probe_o_a_start_in_the_directory_given() {
    let Ok(path) = std::env::var("PROBE_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let dir = FsDir::new(&path)
        .unwrap()
        .with_cache_count(skyzfs_ublk_sys::cached_pages);
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let (store, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
    let outcome = match store.into_in_place(None) {
        Ok(_) => "served".to_string(),
        Err(e) => format!("refused: {e}"),
    };
    eprintln!("probe O: {}: {outcome}", path.display());
}
