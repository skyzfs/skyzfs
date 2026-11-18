
// ---------------------------------------------------------------------
// The review's scenarios for #122 at 70d0f23 (M2.5-1), appended to
// crates/skyzfs-upload/tests/publish.rs for a run (plants.py scenarios).
// A scenario states what should hold. Those that fail at the head show
// a finding of review.md: r1 and r2 (F1), r3 (F2), r8 (F6), r10 (F5).
// ---------------------------------------------------------------------

/// `snap/<name>` read plainly: its commit, the group of that commit, its
/// row, and whether the commit's content is the volume at the instant of
/// the cut the row names. `None` where there is no such ref.
async fn review_named(fx: &Fixture, name: &str) -> Option<(CommitHash, Option<u64>, SnapRow, bool)> {
    let inner = fx.store.inner().clone();
    let refs = ObjectRefStore::new(inner.clone());
    let found = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref")?;
    let row = refs
        .meta_get(fx.vol, &snap_row_key(name))
        .await
        .expect("the row")
        .expect("a row beside the ref");
    let row = SnapRow::decode(&row).expect("the row decodes");
    let engine = Engine::open_at_commit(inner.clone(), refs, fx.vol, found.commit, cfg())
        .await
        .expect("the commit");
    let content = engine.read(0, SIZE).await.expect("the snapshot");
    let same = content == fx.at[&row.cut.group];
    Some((found.commit, engine.head_group(), row, same))
}

/// Step until the cut under way is at its commit.
async fn review_to_the_commit(up: &mut Up) {
    for _ in 0..200 {
        if up.doing() == Doing::Committing {
            return;
        }
        up.step().await.expect("a step");
    }
    panic!("the commit was not reached");
}

/// Cut 1 (group 7) with `pool@t` asked before it: its head swap lands and
/// the reply is lost. The driver is left at the commit.
async fn review_swap_unheard(fx: &mut Fixture) -> (Up, Request, CommitHash) {
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    review_to_the_commit(&mut up).await;
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(up.step().await.is_err(), "the reply to the swap is lost");
    fx.store.disarm().await;
    let cut1 = head(fx).await;
    assert_eq!(fx.uploaded().await.group, Some(7), "cut 1's swap landed");
    (up, asked, cut1)
}

/// F1, the same uploader. Cut 1's swap landed and its reply was lost;
/// before the uploader reads the row and head again, another writer
/// moves head (`Fixture::move_head`: the case `Mismatch::HeadMoved`
/// exists for). The row still names cut 1, so the uploader takes the
/// commit as landed, which is right, and takes head as read then for
/// cut 1's commit, which is not: `snap/t` must not name the other
/// writer's commit.
#[tokio::test]
async fn review_m25_r1_head_moved_after_a_lost_reply_and_snap_t_names_cut_1s_commit_or_nothing() {
    let mut fx = Fixture::new(60).await;
    let (mut up, _, cut1) = review_swap_unheard(&mut fx).await;
    fx.move_head().await;
    let moved = head(&fx).await;
    assert_ne!(moved, cut1);
    let steps = up.run().await.expect("the run after");
    println!("r1: steps {:?}", told(&steps));
    let named = review_named(&fx, "t").await;
    if let Some((commit, group, row, same)) = &named {
        println!(
            "r1: snap/t names {} (cut 1's commit: {}, the other writer's: {}), a commit of group {group:?}; \
             its row names cut {} of group {}; the commit holds the volume at that cut's instant: {same}",
            hex(commit),
            *commit == cut1,
            *commit == moved,
            row.cut.number,
            row.cut.group
        );
    }
    println!("r1: progress {:?}", up.publishes().progress("t"));
    if let Some((commit, _, _, same)) = named {
        assert_eq!(hex(&commit), hex(&cut1), "snap/t names another commit than cut 1's");
        assert!(same);
    }
}

/// F1, a new uploader. As r1, but the daemon dies with the reply lost,
/// head is moved while it is down, and the next uploader, asked again,
/// finds cut 1 committed at its check.
#[tokio::test]
async fn review_m25_r2_head_moved_before_a_restart_and_snap_t_names_cut_1s_commit_or_nothing() {
    let mut fx = Fixture::new(61).await;
    let (up, asked, cut1) = review_swap_unheard(&mut fx).await;
    fx.died(up);
    fx.move_head().await;
    let moved = head(&fx).await;
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the run after");
    println!("r2: steps {:?}", told(&steps));
    let named = review_named(&fx, "t").await;
    if let Some((commit, group, row, same)) = &named {
        println!(
            "r2: snap/t names {} (cut 1's commit: {}, the other writer's: {}), a commit of group {group:?}; \
             its row names cut {} of group {}; the commit holds the volume at that cut's instant: {same}",
            hex(commit),
            *commit == cut1,
            *commit == moved,
            row.cut.number,
            row.cut.group
        );
    }
    println!("r2: progress {:?}", up.publishes().progress("t"));
    if let Some((commit, _, _, same)) = named {
        assert_eq!(hex(&commit), hex(&cut1), "snap/t names another commit than cut 1's");
        assert!(same);
    }
}

fn hex(hash: &CommitHash) -> String {
    hash.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// F2. A request that names another cut state than the store's, asked
/// before the uploader's first check of a cut (the uploader learns the
/// state there, though the store has it from its start): it should be
/// refused in the end, and should not keep the name from a request with
/// the right state.
#[tokio::test]
async fn review_m25_r3_a_request_of_another_cut_state_asked_before_the_first_check_does_not_pend_for_ever() {
    let mut fx = Fixture::new(62).await;
    let store = fx.start();
    let mut up = driver(&fx, store);
    assert_eq!(up.step().await, Ok(Step::Idle), "no cut yet");
    let right = up.local().cut_state_id().expect("the store's cut state");
    let stale = Request {
        name: "t".into(),
        attachment: ATTACHMENT,
        state: *b"the state before",
        after: 3,
        group: 7,
        manifest: manifest("t"),
    };
    assert_ne!(stale.state, right);
    let first = up.publishing().ask(stale.clone());
    println!("r3: the stale request, asked before any cut: {first:?}");
    up.run().await.expect("the lookup");
    for (block, txg) in [(100, 7), (101, 8), (102, 9), (103, 10), (104, 11)] {
        fx.write(up.local(), block, txg as u8);
        fx.group(up.local(), txg);
        up.run().await.expect("a cut");
    }
    fx.assert_uploaded(11, "five cuts later").await;
    let progress = up.publishes().progress("t");
    let again = up.publishing().ask(stale);
    let fresh = request(&mut up, "t", 0, 7);
    let anew = up.publishing().ask(fresh);
    println!(
        "r3: five cuts later: progress {progress:?}; the stale request asked again: {again:?}; \
         the name asked with the store's state: {anew:?}; pending {}",
        up.publishes().pending().0
    );
    assert_ne!(progress, Progress::Pending, "a request no cut of this store can carry pends for ever");
    assert_eq!(anew, Answer::Accepted, "the name is kept from a request with the right state");
}

/// The run of the crash matrix, step by step: the operations on object
/// storage each step makes. For the list of kill points in review.md.
#[tokio::test]
async fn review_m25_r4_the_operations_of_each_step_of_the_matrixs_run() {
    let mut fx = Fixture::new(46).await;
    let mut store = fx.start();
    for block in 60..100 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 16);
    up.publishing().set_attachment(ATTACHMENT);
    let asked = request(&mut up, "t", 0, 7);
    up.publishing().ask(asked);
    fx.store.inner().record(true);
    let mut at = 0;
    loop {
        let step = up.step().await.expect("a step");
        let log = fx.store.inner().log();
        println!("r4: {:>2}..{:>2} {:?}: {:?}", at, log.len(), step, &log[at..]);
        at = log.len();
        if step == Step::Idle {
            break;
        }
    }
    fx.store.inner().record(false);
}

/// What one failure in [`review_older`] showed.
struct ReviewSeen {
    died: bool,
    landed: bool,
    on: u64,
}

/// A publish on a cut that is no longer head: cuts 1 and 2 are committed,
/// then `pool@t` is asked with C = 0, so that it goes on cut 1 while head
/// is cut 2's. Object storage fails at the `k`th operation from the ask.
async fn review_older(when: FaultWhen, k: u64, back: Back) -> ReviewSeen {
    quiet_deaths();
    let case = format!("older {when:?} {k} {back:?}");
    let mut fx = Fixture::new(63).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let cut1 = head(&fx).await;
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2");
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    fx.store.arm_when(FaultOp::Any, "", k, when).await;
    let ran = up.run().await;
    fx.store.disarm().await;
    if let Ok(steps) = &ran {
        assert!(
            matches!(steps.last(), Some(Step::Published { found: false, number: 1, .. })),
            "{case}: {steps:?}"
        );
        let done = published(&fx, "t", &case).await.expect("published");
        assert_eq!(done.commit, cut1, "{case}");
        return ReviewSeen {
            died: false,
            landed: true,
            on: 1,
        };
    }
    let left = published(&fx, "t", &format!("{case}, as the failure left it")).await;
    if let Some(left) = &left {
        assert_eq!(left.commit, cut1, "{case}");
        assert_eq!(left.row.cut.number, 1, "{case}");
    }
    assert!(fx.uploaded().await.fsck, "{case}: fsck");
    let mut up = match back {
        Back::Retry => up,
        Back::Restart => {
            fx.died(up);
            let mut up = driver(&fx, fx.start());
            assert_eq!(up.publishing().ask(asked), Answer::Accepted, "{case}");
            up
        }
    };
    up.run()
        .await
        .unwrap_or_else(|e| panic!("{case}: the run after: {e}"));
    if up.publishes().progress("t") == Progress::Pending {
        assert!(matches!(back, Back::Restart) && left.is_none(), "{case}: pending");
        fx.write(up.local(), 100, 0x09);
        fx.group(up.local(), 9);
        up.run().await.expect("cut 3");
    }
    let done = published(&fx, "t", &format!("{case}, after"))
        .await
        .unwrap_or_else(|| panic!("{case}: not published"));
    assert_eq!(up.publishes().progress("t"), Progress::Done(done.commit), "{case}");
    let landed = left.is_some();
    if let Some(left) = left {
        assert_eq!(left.commit, done.commit, "{case}: the ref moved");
        assert_eq!(left.row, done.row, "{case}: the row changed");
    }
    assert_eq!(up.figures().mismatches, 0, "{case}");
    ReviewSeen {
        died: true,
        landed,
        on: done.row.cut.number,
    }
}

/// The matrix again for a publish on a cut that is not head (the pull
/// request's matrix publishes head's cut only): after a failure at any
/// operation, before it or after it, `snap/t` is absent or whole on cut
/// 1; the same uploader publishes it on cut 1; a new one, which knows no
/// cut, on the next cut, 3.
#[tokio::test]
async fn review_m25_r5_a_failure_at_any_operation_of_a_publish_on_an_older_cut() {
    for back in [Back::Retry, Back::Restart] {
        for when in [FaultWhen::Before, FaultWhen::After] {
            let mut points = 0;
            let mut left = 0;
            let mut on = Vec::new();
            for k in 0.. {
                let seen = review_older(when, k, back).await;
                if !seen.died {
                    break;
                }
                points += 1;
                left += u64::from(seen.landed);
                on.push(seen.on);
            }
            println!("r5: {back:?} {when:?}: {points} points, the ref left by {left}, published on cuts {on:?}");
            assert!(points >= 4, "{back:?} {when:?}: {points}");
            let expected = match when {
                FaultWhen::Before => 0,
                FaultWhen::After => 1,
            };
            assert_eq!(left, expected, "{back:?} {when:?}");
        }
    }
}

/// Two failures in a row: the publish's transaction lands and its reply
/// is lost, and the lookup after it fails too. The uploader looks again
/// and finds its own publish; no second transaction is made, and the ref
/// does not move.
#[tokio::test]
async fn review_m25_r6_a_lost_reply_and_then_a_failed_lookup() {
    let mut fx = Fixture::new(64).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    // The second PUT of the refs object is the publish's transaction.
    fx.store
        .arm_when(FaultOp::Put, "refs", 1, FaultWhen::After)
        .await;
    assert!(up.run().await.is_err(), "the reply to the publish is lost");
    fx.store.disarm().await;
    let left = published(&fx, "t", "r6, as the failure left it")
        .await
        .expect("it landed");
    fx.store.arm(FaultOp::Any, "", 0).await;
    assert!(up.step().await.is_err(), "the lookup fails");
    assert_eq!(up.publishes().progress("t"), Progress::Pending);
    fx.store.disarm().await;
    fx.store.inner().record(true);
    let steps = up.run().await.expect("the run after");
    fx.store.inner().record(false);
    assert_eq!(
        steps,
        [Step::Published {
            name: "t".into(),
            number: 1,
            commit: left.commit,
            found: true
        }]
    );
    let log = fx.store.inner().log();
    assert!(!log.iter().any(|op| op.starts_with("put")), "nothing is written again: {log:?}");
    let after = published(&fx, "t", "r6, after").await.expect("published");
    assert_eq!((after.commit, after.row), (left.commit, left.row));
}

/// A cut the bound took after its commit had landed and before it was
/// let go (`committed_then_taken`): the reply to its swap was lost, the
/// row read again names it, and it is no longer held. A publish it
/// carries goes on its commit, which holds the volume at its instant.
#[tokio::test]
async fn review_m25_r7_a_cut_taken_after_its_commit_carries_its_publish() {
    let mut fx = Fixture::new(65).await;
    let mut up = committed_then_taken(&mut fx).await;
    up.publishing().set_attachment(ATTACHMENT);
    let cut2 = head(&fx).await;
    let asked = request(&mut up, "t", 1, 8);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the run");
    println!("r7: steps {:?}", told(&steps));
    assert!(
        steps.contains(&Step::Published {
            name: "t".into(),
            number: 2,
            commit: cut2,
            found: false
        }),
        "{steps:?}"
    );
    let done = published(&fx, "t", "r7").await.expect("published");
    assert_eq!((done.commit, done.row.cut.number, done.row.cut.group), (cut2, 2, 8));
    fx.assert_uploaded(9, "cut 3 after it").await;
}

/// F6. `snap/t` was published on cut 1 and is then deleted as SkyNBD
/// deletes a snapshot (the ref alone: the row `skyzfs/snap/t` stays), and
/// the name is made again by another on another commit. A new uploader
/// asked for the first publish again should not answer that `snap/t` is
/// its publish: the ref names a commit the row says nothing about.
#[tokio::test]
async fn review_m25_r8_a_ref_made_anew_by_another_beside_a_row_left_behind_is_not_ours() {
    let mut fx = Fixture::new(66).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    let cut1 = published(&fx, "t", "r8, first").await.expect("published").commit;
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2");
    let cut2 = head(&fx).await;
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let current = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref");
    for (expected, new_commit) in [(current, None), (None, Some(cut2))] {
        refs.transact(
            fx.vol,
            vec![RefOp::CasRef {
                name: snap_ref("t"),
                expected,
                new_commit,
            }],
        )
        .await
        .expect("another's delete, then another's snapshot");
    }
    let row = refs.meta_get(fx.vol, &snap_row_key("t")).await.expect("the row");
    println!("r8: the row left behind by the delete: {}", row.is_some());
    fx.died(up);
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    println!(
        "r8: steps {steps:?}; snap/t names cut 2's commit: {}, cut 1's: {}",
        refs.get(fx.vol, &snap_ref("t")).await.expect("the ref").map(|v| v.commit) == Some(cut2),
        refs.get(fx.vol, &snap_ref("t")).await.expect("the ref").map(|v| v.commit) == Some(cut1),
    );
    assert_eq!(
        steps,
        [Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        }]
    );
}

/// The name is taken by another between the lookup, which found it
/// absent, and the publish's transaction: the transaction is refused,
/// the lookup after it finds the other's ref, the publish ends refused,
/// and the cuts go on.
#[tokio::test]
async fn review_m25_r9_a_name_taken_between_the_lookup_and_the_transaction() {
    let mut fx = Fixture::new(67).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let cut1 = head(&fx).await;
    assert_eq!(ask_now(&mut up, "t", 8), Answer::Accepted);
    assert_eq!(up.step().await, Ok(Step::PublishLooked { name: "t".into() }));
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref("t"),
            expected: None,
            new_commit: Some(cut1),
        }],
    )
    .await
    .expect("another's snapshot");
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    let mut steps = Vec::new();
    let mut errors = 0;
    for _ in 0..50 {
        match up.step().await {
            Ok(Step::Idle) => break,
            Ok(step) => steps.push(step),
            Err(e) => {
                errors += 1;
                assert_eq!(e, skyzfs_upload::RemoteError::Conflict);
            }
        }
    }
    println!("r9: steps {:?}, errors {errors}", told(&steps));
    assert_eq!(errors, 1, "the transaction is refused once");
    assert_eq!(
        told(&steps).last(),
        Some(&Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        })
    );
    assert_eq!(up.publishes().progress("t"), Progress::Refused(Refusal::Taken));
    let theirs = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref").expect("theirs");
    assert_eq!(theirs.commit, cut1, "their ref is as they made it");
    assert_eq!(
        refs.meta_get(fx.vol, &snap_row_key("t")).await.expect("the row"),
        None,
        "no row of ours beside their ref"
    );
    fx.assert_uploaded(8, "cut 2").await;
}

/// F5. `snap/t` was published and is then deleted in object storage
/// (the ref, as retention does, design §5.2), and the pool takes a new
/// snapshot of the same name, with another manifest. The same uploader,
/// asked for it, should look the name up as it does any name it has no
/// publish pending for; it answers from what it remembers instead.
#[tokio::test]
async fn review_m25_r10_a_name_deleted_and_asked_again_is_looked_up_not_remembered() {
    let mut fx = Fixture::new(68).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    let first = published(&fx, "t", "r10, first").await.expect("published").commit;
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let current = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref");
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref("t"),
            expected: current,
            new_commit: None,
        }],
    )
    .await
    .expect("the delete");
    let same = up.publishing().ask(asked.clone());
    println!(
        "r10: the ref is gone; the same request asked again answers {same:?} (Done names the first commit: {})",
        same == Answer::Done(first)
    );
    let mut anew = asked;
    anew.after = up.local().cuts().count;
    anew.group = 8;
    anew.manifest = b"another snapshot of the same name".to_vec();
    let answer = up.publishing().ask(anew);
    println!("r10: a new snapshot of the name asked: {answer:?}");
    assert_eq!(answer, Answer::Accepted, "the name is free in object storage");
}
