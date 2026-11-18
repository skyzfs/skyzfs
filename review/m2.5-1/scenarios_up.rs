
// ---------------------------------------------------------------------
// The review's scenarios for #122 at 70d0f23 (M2.5-1) through the
// uploader's loop, appended to crates/skyzfs-upload/tests/uploader.rs for
// a run (plants.py scenarios). The pull request has no test of a publish
// through `Uploader`. u3 and u4 state what should hold and fail at the
// head: F3 of review.md. u5 prints what F4 says and asserts nothing of it.
// ---------------------------------------------------------------------

const REVIEW_ATTACHMENT: [u8; 16] = *b"the attachment..";

fn review_request(state: [u8; 16], name: &str, after: u64, group: u64) -> skyzfs_upload::Request {
    skyzfs_upload::Request {
        name: name.into(),
        attachment: REVIEW_ATTACHMENT,
        state,
        after,
        group,
        manifest: format!("the ZFS manifest of pool@{name}").into_bytes(),
    }
}

/// `snap/<name>` and its row, as the refs object has them.
async fn review_snap(fx: &Fixture, name: &str) -> (Option<CommitHash>, Option<Vec<u8>>) {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let found = refs
        .get(fx.vol, &format!("snap/{name}"))
        .await
        .expect("the ref")
        .map(|value| value.commit);
    let row = refs
        .meta_get(fx.vol, &format!("skyzfs/snap/{name}"))
        .await
        .expect("the row");
    (found, row)
}

async fn review_head(fx: &Fixture) -> CommitHash {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.get(fx.vol, "head").await.expect("head").expect("a head").commit
}

/// A publish asked before the lease is held, and so before the driver is
/// made, is made once the lease is held and the cut is committed.
#[tokio::test(start_paused = true)]
async fn review_m25_u1_a_publish_asked_before_the_lease_is_made_once_it_is_held() {
    let mut fx = Fixture::new(71).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    up.publishing().set_attachment(REVIEW_ATTACHMENT);
    let wait = up.turn().await;
    assert!(matches!(up.shown().state, State::NoLease(_)));
    assert!(up.driver().is_none());
    assert_eq!(
        up.publishing().ask(review_request(state, "t", 0, 7)),
        skyzfs_upload::Answer::Accepted
    );
    assert_eq!(up.publishing().progress("t"), skyzfs_upload::Progress::Pending);
    fx.store.disarm().await;
    tokio::time::advance(wait).await;
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    let head = review_head(&fx).await;
    assert_eq!(up.publishing().progress("t"), skyzfs_upload::Progress::Done(head));
    let (found, row) = review_snap(&fx, "t").await;
    assert_eq!(found, Some(head));
    assert!(row.is_some());
}

/// The publish's transaction is fenced by the lease's epoch: after
/// another holder has taken the lease, a publish by this uploader lands
/// nothing and loses the lease, as a commit does.
#[tokio::test(start_paused = true)]
async fn review_m25_u2_a_publish_after_another_took_the_lease_is_fenced_and_lands_nothing() {
    let mut fx = Fixture::new(72).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    up.publishing().set_attachment(REVIEW_ATTACHMENT);
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    clock.pass(TTL + 1);
    let other = clock
        .refs(&fx)
        .lease_acquire(fx.vol, "node-b", TTL)
        .await
        .expect("the other's take");
    assert!(matches!(other, LeaseOutcome::Acquired(l) if l.epoch == 2));
    let asked = review_request(state, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), skyzfs_upload::Answer::Accepted);
    fx.store.inner().record(true);
    let mut states = Vec::new();
    for _ in 0..20 {
        up.turn().await;
        states.push(up.shown().state);
        if matches!(states.last(), Some(State::LeaseLost { .. })) {
            break;
        }
    }
    fx.store.inner().record(false);
    assert_eq!(
        states.last(),
        Some(&State::LeaseLost {
            holder: "node-b".into(),
            epoch: 2
        }),
        "{states:?}"
    );
    let log = fx.store.inner().log();
    println!("u2: the operations of the fenced publish: {log:?}");
    assert_eq!(
        log.iter().filter(|op| op.starts_with("put_if")).count(),
        0,
        "the fence refused the transaction before any write of the refs object"
    );
    assert_eq!(review_snap(&fx, "t").await, (None, None), "nothing landed");
    // What the asker sees from then on (F4): the publish pends for good.
    for _ in 0..5 {
        tokio::time::advance(AT_REST).await;
        up.turn().await;
    }
    println!(
        "u2: after the lease is lost for good: progress {:?}; asked again: {:?}; a new name: {:?}",
        up.publishing().progress("t"),
        up.publishing().ask(asked),
        up.publishing().ask(review_request(state, "u", 0, 7)),
    );
}

/// F3. The PUT of a ZFS manifest fails every time, and nothing else does
/// (a store that refuses that key: a policy on the prefix, say). The
/// publish cannot be made. The cuts should still go up: design §5.2 lets
/// the cut give, never the recovery point wait on a snapshot's name. And
/// the attempts should come at the backoff's growing waits.
#[tokio::test(start_paused = true)]
async fn review_m25_u3_a_publish_that_cannot_be_made_does_not_stop_the_cuts() {
    let mut fx = Fixture::new(73).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    up.publishing().set_attachment(REVIEW_ATTACHMENT);
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;

    fx.store.arm(FaultOp::Put, "skyzfs/manifest", 0).await;
    assert_eq!(
        up.publishing().ask(review_request(state, "t", 0, 7)),
        skyzfs_upload::Answer::Accepted
    );
    // Cut 2, to be uploaded.
    {
        let local = up.driver().expect("the driver").local();
        fx.write(local, 20, 0x20);
        fx.group(local, 8);
    }
    let began = tokio::time::Instant::now();
    let mut waits: Vec<Duration> = Vec::new();
    let mut turns = 0u64;
    while began.elapsed() < Duration::from_secs(600) {
        let wait = up.turn().await;
        turns += 1;
        if let State::Backoff { wait, .. } = up.shown().state {
            waits.push(wait);
        }
        // The lease is renewed on its own clock.
        clock.pass(wait.as_millis() as u64);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    let shown = up.shown();
    let longest = waits.iter().max().copied().unwrap_or_default();
    let group = fx.uploaded().await.group;
    println!(
        "u3: in 600 s with every PUT of the ZFS manifest failing: {turns} turns, {} waits after an error, \
         the longest {longest:?}; errors {}; head's group {group:?} (cut 2 is group 8); cuts uploaded {}; state {:?}",
        waits.len(),
        shown.figures.errors,
        shown.figures.uploaded,
        shown.state
    );
    assert_eq!(group, Some(8), "cut 2 did not go up in ten minutes");
    assert!(longest > Duration::from_secs(1), "the waits did not grow");
}

/// A remote whose publish is refused with a conflict while the lookup
/// finds no ref: what a refs store that fences writes answers while the
/// volume has no lease row (`LeaseLost` with no holder, which the engine
/// maps to a conflict; `RemoteError::Conflict`'s own note).
struct ReviewNoRow<R> {
    inner: R,
    publishes: Arc<AtomicU64>,
}

impl<R: Remote> Remote for ReviewNoRow<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        self.inner.observe().await
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }
    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }
    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        self.inner.put_sealed().await
    }
    async fn commit(
        &mut self,
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<CommitHash, RemoteError> {
        self.inner.commit(group, expected, row).await
    }
    async fn snapshot(&mut self, _name: &str) -> Result<Option<Found>, RemoteError> {
        Ok(None)
    }
    async fn publish(
        &mut self,
        _commit: CommitHash,
        _snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        self.publishes.fetch_add(1, Ordering::SeqCst);
        Err(RemoteError::Conflict)
    }
    fn discard(&mut self) {
        self.inner.discard()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// F3, the conflict. A publish refused with a conflict, again and again,
/// while the cut under way stands at its commit: the uploader should wait
/// between the attempts, as it does for a commit refused again and again
/// (G7 of #114's review).
#[tokio::test(start_paused = true)]
async fn review_m25_u4_a_publish_refused_again_and_again_is_waited_after() {
    let mut fx = Fixture::new(74).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let publishes = Arc::new(AtomicU64::new(0));
    let mut make = fenced(&fx, &clock);
    let counter = Arc::clone(&publishes);
    let mut up = Uploader::with_slices(
        store,
        keeper(&fx, &clock),
        move |epoch| ReviewNoRow {
            inner: make(epoch),
            publishes: Arc::clone(&counter),
        },
        16,
    );
    up.publishing().set_attachment(REVIEW_ATTACHMENT);
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    // Cut 2 is read up to its commit; then the publish is asked, which
    // cut 1 carries.
    {
        let local = up.driver().expect("the driver").local();
        fx.write(local, 20, 0x20);
        fx.group(local, 8);
    }
    for _ in 0..100 {
        if up.driver().expect("the driver").doing() == Doing::Committing {
            break;
        }
        up.turn().await;
    }
    assert_eq!(up.driver().expect("the driver").doing(), Doing::Committing);
    assert_eq!(
        up.publishing().ask(review_request(state, "t", 0, 7)),
        skyzfs_upload::Answer::Accepted
    );
    let mut waited = Duration::ZERO;
    let turns = 2000;
    for _ in 0..turns {
        waited += up.turn().await;
    }
    let made = publishes.load(Ordering::SeqCst);
    println!(
        "u4: {turns} turns asked for {waited:?} of waiting in all; the publish was sent {made} times; \
         head's group {:?}; state {:?}",
        fx.uploaded().await.group,
        up.shown().state
    );
    assert!(
        waited >= Duration::from_secs(made.saturating_sub(1)),
        "{made} publishes sent with {waited:?} of waiting between them"
    );
}

/// F4, printed. A cut is committed and the store keeps it (the intent
/// map's pages cannot be written: the volume has failed): the driver
/// stops, and the uploader steps it no more. A publish that cut carries,
/// ready on a commit that is whole, is never made, and pends.
#[tokio::test(start_paused = true)]
async fn review_m25_u5_what_a_publish_does_after_the_store_kept_its_cut() {
    let mut fx = Fixture::new(75).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    up.publishing().set_attachment(REVIEW_ATTACHMENT);
    assert_eq!(
        up.publishing().ask(review_request(state, "t", 0, 7)),
        skyzfs_upload::Answer::Accepted
    );
    for _ in 0..100 {
        up.turn().await;
        if up.driver().is_some_and(|d| d.doing() == Doing::Committing) {
            break;
        }
    }
    fx.disk.fail_write_of("intent.map");
    for _ in 0..10 {
        let wait = up.turn().await;
        tokio::time::advance(wait).await;
    }
    fx.assert_uploaded(7, "cut 1 is committed").await;
    println!(
        "u5: the store kept cut 1, committed: state {:?}; kept {}; progress of t {:?}; snap/t {:?}; \
         a step of the driver alone would be {:?}",
        up.shown().state,
        up.shown().figures.kept,
        up.publishing().progress("t"),
        review_snap(&fx, "t").await.0.is_some(),
        up.driver().expect("the driver").step().await,
    );
}
