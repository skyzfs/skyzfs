// Review scratch test for skyzfs/skyzfs#150: appended for a run to the end
// of bins/skyzfsd/src/upload/tests.rs at the head under review, then the
// file is restored (run150.sh). The pull request's own
// `the_carry_hook_passes_the_channel_on_and_counts_the_publishes_accepted`
// drops the asker of "w" after `blocking_send` has put the ask in the
// inbox, so the hook's forwarder, on the uploader's thread, can pass it on
// before the drop: the uploader then takes "w" and the test fails at
// "not taken" (measured: 2 in 2000 and 2 in 4000 runs). Here the asker is
// dropped before the send, which is what the test means; the rest is the
// same.

#[cfg(all(target_os = "linux", feature = "test-hooks"))]
#[test]
fn r150_the_carry_hook_skips_an_ask_whose_asker_went_before_it_was_sent() {
    use skyzfs_upload::publish::{Request as Ask, EXPIRY_DEFAULT};
    use skyzfs_upload::{Answer, Control, Progress};
    let world = World::new();
    let (requests, inbox) = tokio::sync::mpsc::channel(4);
    let publishes = Publishes {
        attachment: Some([0xa1; 16]),
        state: Some([0x5e; 16]),
        rewound: 0,
        inbox,
        told: Box::new(|_, _| {}),
    };
    let carry = crate::kill::Carry::new(
        Some(crate::cli::KillAt::CarrySwap(1)),
        std::sync::Arc::new(|_: &str| {}),
    )
    .expect("a carry hook");
    let (up, posts, queue, _) = uploading_around(
        &world,
        true,
        |_| {},
        publishes,
        std::sync::Arc::clone(&carry),
    );
    idle_and_held(&posts);
    fn within<T>(mut answer: tokio::sync::oneshot::Receiver<T>) -> Option<T> {
        for _ in 0..1000 {
            if let Ok(got) = answer.try_recv() {
                return Some(got);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
    let ask = |name: &str| Ask {
        name: name.into(),
        attachment: [0xa1; 16],
        state: [0x5e; 16],
        after: 0,
        group: 1,
        manifest: Vec::new(),
        expiry: EXPIRY_DEFAULT,
    };
    let (reply, answer) = tokio::sync::oneshot::channel();
    requests
        .blocking_send(Control::Ask {
            request: ask("t"),
            rewound: 0,
            reply,
        })
        .expect("sent");
    assert_eq!(within(answer), Some(Answer::Accepted));
    // The asker goes before its ask is sent.
    let (reply, answer) = tokio::sync::oneshot::channel::<Answer>();
    drop(answer);
    requests
        .blocking_send(Control::Ask {
            request: ask("w"),
            rewound: 0,
            reply,
        })
        .expect("sent");
    let progress = |name: &str| {
        let (reply, answer) = tokio::sync::oneshot::channel();
        requests
            .blocking_send(Control::Progress(name.into(), reply))
            .expect("sent");
        within(answer)
    };
    assert_eq!(progress("t"), Some(Progress::Pending));
    assert_eq!(progress("w"), Some(Progress::Unknown), "not taken");
    assert_eq!(carry.accepted(), 1);
    assert_eq!(up.end(false, END_LIMIT), Ended::Kept);
    queue.join().expect("the queue's thread");
}
