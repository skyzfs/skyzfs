
    /// The review's scenarios for #136, appended to `fs.rs`'s tests for a
    /// run. (a) As the store does on a failed volume: a sync is begun and
    /// the file dropped at once, before the helper may have taken the
    /// job up; the drop returns only once the sync was made and its wake
    /// ran. (b) A second sync queued behind one whose wake is slow: the
    /// drop still makes it.
    #[test]
    fn review_q7b_a_sync_begun_just_before_the_drop_is_made() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        // (a)
        let mut file = dir.create("a").unwrap();
        file.write_at(0, &[7; 512]).unwrap();
        let ended = Arc::new(AtomicBool::new(false));
        let flag = ended.clone();
        file.sync_begin(Box::new(move || {
            thread::sleep(std::time::Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }))
        .unwrap();
        drop(file);
        assert!(ended.load(Ordering::SeqCst), "(a) the drop did not make the sync begun just before it");
        // (b)
        let mut file = dir.create("b").unwrap();
        file.write_at(0, &[8; 512]).unwrap();
        let (first, second) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (f1, f2) = (first.clone(), second.clone());
        let (begun, started) = mpsc::channel();
        file.sync_begin(Box::new(move || {
            let _ = begun.send(());
            thread::sleep(std::time::Duration::from_millis(100));
            f1.store(true, Ordering::SeqCst);
        }))
        .unwrap();
        started.recv().unwrap();
        file.sync_begin(Box::new(move || {
            f2.store(true, Ordering::SeqCst);
        }))
        .unwrap();
        drop(file);
        assert!(first.load(Ordering::SeqCst), "(b) the first");
        assert!(second.load(Ordering::SeqCst), "(b) the sync queued behind it");
    }
}
