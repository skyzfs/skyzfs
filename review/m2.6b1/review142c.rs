// Scratch cases of the next look at #142 at 347a17f (Codex's second P2,
// `parent_of`), included at the end of record/tests.rs in a scratch
// worktree only. Each asserts what the code DOES at the head.

mod review142c {
    use super::*;

    /// The directory `make_dir` syncs, for the forms a store path can take.
    /// Rust's `Path::parent` drops a trailing `/` and an inner `.`; `..`
    /// and `.` are components with an empty parent.
    #[test]
    fn review_p_parent_of_each_form() {
        let p = |s: &str| parent_of(Path::new(s)).map(|p| p.to_str().unwrap_or("?").to_string());
        let s = |x: &str| Some(x.to_string());
        assert_eq!(p("agent"), s("."));
        assert_eq!(p("agent/"), s("."));
        assert_eq!(p("agent/."), s("."));
        assert_eq!(p("./agent"), s("."));
        assert_eq!(p("../agent"), s(".."));
        assert_eq!(p("a/b/agent/"), s("a/b"));
        assert_eq!(p("/var/lib/agent/"), s("/var/lib"));
        assert_eq!(p("/agent"), s("/"));
        assert_eq!(p("/"), None);
        assert_eq!(p("//"), None);
        // A directory that names an existing one: `.` and `..` give `.`,
        // `a/..` gives `a`. Each exists already (create_dir: AlreadyExists),
        // so which directory is synced does not matter there.
        assert_eq!(p("."), s("."));
        assert_eq!(p(".."), s("."));
        assert_eq!(p("a/.."), s("a"));
        // An empty path: no parent; create_dir("") fails first (NotFound).
        assert_eq!(p(""), None);
        assert_eq!(
            make_dir(Path::new("")).map_err(|e| e.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }

    /// A save from a store of one relative part, run in a temporary
    /// current directory: the store and its record are there and load.
    /// Run alone (`--exact`) under strace to see which directories are
    /// synced; it changes the process's current directory.
    #[test]
    #[ignore = "changes the current directory; run alone"]
    fn review_p_save_from_a_relative_store() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_current_dir(tmp.path()).expect("cd");
        let store = Store::new("agent");
        store.save(&Record::new([7; 16])).expect("saved");
        store.save(&Record::new([7; 16])).expect("saved again");
        assert!(matches!(store.load(), Ok(Loaded::Present(_))));
        std::env::set_current_dir("/").expect("cd /");
    }
}
