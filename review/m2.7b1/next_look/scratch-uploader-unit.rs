
    /// Scratch S5 (the review of #175, its next look): a retire given up
    /// after its tries is worded as a retire's conflict, naming object
    /// storage once; any other error keeps its own text. Mutants: the
    /// conflict worded as a publish's (`conflict-text-generic`), or with
    /// no "object storage" (`conflict-text-noprefix`).
    #[test]
    fn review_s5_a_retire_that_gave_up_is_worded_as_a_retire() {
        let text = retire_failed(&RemoteError::Conflict);
        assert_eq!(
            text,
            format!(
                "object storage: snap/T moved under each of {} tries",
                crate::remote::RETIRE_TRIES
            )
        );
        assert_eq!(
            retire_failed(&RemoteError::Store("down".into())),
            "object storage: down"
        );
    }
