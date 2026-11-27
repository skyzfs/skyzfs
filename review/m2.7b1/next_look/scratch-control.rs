
    /// Scratch S4 (the review of #175, its next look): a request that the
    /// uploader took and dropped unanswered reads `ended` with the advice
    /// of its verb, not the bare "the uploader has ended". Mutant: the
    /// bare text (`ended-text`).
    #[test]
    fn review_s4_a_request_the_uploader_dropped_reads_ended_with_advice() {
        let (mut handler, thread) = uploader_with(|control, _| drop(control));
        let retire = Request::Retire(Retire {
            name: "t".into(),
            commit: [0xc0; 32],
        });
        let words = [
            handler(&retire, Vec::new()).line(),
            handler(&publish(None), b"12345".to_vec()).line(),
        ];
        for line in &words {
            assert!(
                line.starts_with("refused ended the uploader has ended; "),
                "{line}"
            );
        }
        assert!(words[0].contains("retire"), "{}", words[0]);
        assert!(words[1].contains("publish"), "{}", words[1]);
        drop(handler);
        let _ = thread.join();
    }
