
// ---- review probe (not part of the pull request) ----

/// Probe S: the home image mapped and touched by another process when
/// the store starts, in the directory PROBE_DIR names. The drop cannot
/// take mapped pages. Does the start refuse?
#[test]
fn probe_s_a_start_with_the_image_mapped_by_another_process() {
    let Ok(path) = std::env::var("PROBE_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let dir = on_disk(&path);
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let mut store = start(&dir).unwrap();
    write(&mut store, 0, Buf::filled(4, 0x11));
    flush(&mut store);
    drop(store);
    let image = path.join("home.img");
    let mut child = Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(
            "import mmap, os, sys\n\
             fd = os.open(sys.argv[1], os.O_RDONLY)\n\
             m = mmap.mmap(fd, 0, mmap.MAP_SHARED, mmap.PROT_READ)\n\
             s = sum(m[i] for i in range(0, len(m), 4096))\n\
             print('ready', flush=True)\n\
             sys.stdin.readline()\n",
        )
        .arg(&image)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let below = std::env::var("PROBE_BELOW").ok().map(|p| {
        let f = std::fs::File::open(std::path::Path::new(&p).join("home.img")).unwrap();
        skyzfs_ublk_sys::cached_pages(&f).unwrap()
    });
    let (store, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
    let outcome = match store.into_in_place(None) {
        Ok(_) => "served".to_string(),
        Err(e) => format!("refused: {e}"),
    };
    let after = std::env::var("PROBE_BELOW").ok().map(|p| {
        let f = std::fs::File::open(std::path::Path::new(&p).join("home.img")).unwrap();
        skyzfs_ublk_sys::cached_pages(&f).unwrap()
    });
    eprintln!(
        "probe S: {}: {outcome}; pages of the file below in the cache before the start {below:?}, after it {after:?}",
        path.display()
    );
    drop(child.stdin.take());
    let _ = child.wait();
}
