// Appended to crates/skyzfs-upload/tests/uploader.rs for a run (not part of the pull request).

// ---- review scenarios (not part of the pull request) ----

/// A remote whose every PUT of sealed segments takes `stall` first.
struct Stalling<R> {
    inner: R,
    stall: Duration,
}

impl<R: Remote> Remote for Stalling<R> {
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
        eprintln!("review: put_sealed begins at {:?}", tokio::time::Instant::now());
        tokio::time::sleep(self.stall).await;
        eprintln!("review: put_sealed ends at {:?}", tokio::time::Instant::now());
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
    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        self.inner.snapshot(name).await
    }
    async fn publish(&mut self, commit: CommitHash, snapshot: &Snapshot<'_>) -> Result<(), RemoteError> {
        self.inner.publish(commit, snapshot).await
    }
    fn discard(&mut self) {
        self.inner.discard()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// Review: while a step waits on object storage, the count of cuts is not
/// answered; the daemon's handler gives up after its 30 s.
#[tokio::test(start_paused = true)]
async fn review_the_count_waits_for_the_step_under_way() {
    let mut fx = Fixture::new(91).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut make = fenced(&fx, &clock);
    let stall = Duration::from_secs(60);
    let mut up = Uploader::with_slices(
        store,
        keeper(&fx, &clock),
        move |epoch| Stalling { inner: make(epoch), stall },
        16,
    );
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (inbox, requests) = tokio::sync::mpsc::channel(4);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let client = async move {
        // Into the first stalled PUT.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let began = tokio::time::Instant::now();
        let (reply, answer) = tokio::sync::oneshot::channel();
        inbox.send(Control::Cuts(reply)).await.expect("sent");
        let within = tokio::time::timeout(Duration::from_secs(30), answer).await;
        let timed_out = within.is_err();
        // Asked again, waited for without a limit.
        let (reply, answer) = tokio::sync::oneshot::channel();
        eprintln!("review: second asked at {:?}", tokio::time::Instant::now());
        inbox.send(Control::Cuts(reply)).await.expect("sent");
        let _ = answer.await;
        eprintln!("review: second answered at {:?}", tokio::time::Instant::now());
        let took = began.elapsed();
        let _ = stop.send(());
        (timed_out, took)
    };
    let (_keeper, (timed_out, took)) = tokio::join!(run, client);
    eprintln!("review: timed out within 30 s: {timed_out}; answered after {took:?}");
    assert!(timed_out, "the count was answered during the stalled step");
}

/// Review: a store started with cuts held names its cut state before its
/// first cut, so the daemon's uploader knows it from the start.
#[tokio::test(start_paused = true)]
async fn review_the_cut_state_is_named_before_the_first_cut() {
    let mut fx = Fixture::new(92).await;
    let store = fx.start();
    let now = skyzfs_upload::Store::cuts(&mut { store });
    eprintln!("review: before the first cut: {now:?}");
    assert!(now.state.is_some());
    assert_eq!(now.count, 0);
}

/// A store whose answer to `cuts` takes `delay` ms of tokio's time.
struct SlowCount<L> {
    inner: L,
    delay: Arc<AtomicU64>,
}

impl<L: skyzfs_upload::Local> skyzfs_upload::Local for SlowCount<L> {
    async fn uploadable(&mut self) -> Option<skyzfs_upload::Uploadable> {
        self.inner.uploadable().await
    }
    async fn read(
        &mut self,
        number: u64,
        from: u64,
        buf: skyzfs_blk::Buf,
    ) -> (skyzfs_blk::Buf, std::io::Result<skyzfs_local::inplace::CutSlice>) {
        self.inner.read(number, from, buf).await
    }
    async fn sync_copies(&mut self, number: u64) -> std::io::Result<()> {
        self.inner.sync_copies(number).await
    }
    async fn release(&mut self, number: u64) -> skyzfs_upload::Released {
        self.inner.release(number).await
    }
    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        self.inner.name_committed(number, group).await
    }
    async fn abandon_all(&mut self) -> Vec<u64> {
        self.inner.abandon_all().await
    }
    async fn cuts(&mut self) -> skyzfs_upload::CutsNow {
        let ms = self.delay.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.inner.cuts().await
    }
}

/// Review: a channel request (`cuts`, and `publish` since 460e67c) whose
/// store answer takes 21 s, asked a quarter second before a renewal is
/// due (#117's scenario C2, through the channel): is the renewal made
/// beside it?
#[tokio::test(start_paused = true)]
async fn review_a_slow_store_answer_to_a_channel_request_and_the_lease() {
    let mut lapsed = Vec::new();
    for (seed, which) in [(93, "cuts"), (94, "publish")] {
        let fx = Fixture::new(seed).await;
        let delay = Arc::new(AtomicU64::new(0));
        let store = fx.start();
        let state = store.cut_state_id().expect("a state");
        let slow = SlowCount {
            inner: store,
            delay: Arc::clone(&delay),
        };
        let clock = Clock::new();
        let way = Leasing::apart(&fx);
        let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
        *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
        let (inbox, requests) = tokio::sync::mpsc::channel(4);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let run = up.run_with(
            async move {
                let _ = stopped.await;
            },
            requests,
            |_| {},
        );
        let w = way.clone();
        let d = Arc::clone(&delay);
        let client = async move {
            let begin = Duration::from_millis(TTL / 3) - IDLE;
            tokio::time::sleep(begin - w.elapsed()).await;
            d.store(21_000, Ordering::SeqCst);
            match which {
                "cuts" => {
                    let _ = ask_of(&inbox, Control::Cuts).await;
                }
                _ => {
                    let _ = ask_of(&inbox, |r| Control::Ask(asked(state, "t", 0, 100), r)).await;
                }
            }
            d.store(0, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(60)).await;
            let _ = stop.send(());
        };
        let _ = tokio::join!(run, client);
        let takes = way.takes();
        let gaps: Vec<Duration> = takes.windows(2).map(|w| w[1] - w[0]).collect();
        let longest = gaps.iter().max().copied().unwrap_or_default();
        eprintln!("review: {which}: takes {takes:?}; longest gap {longest:?} (ttl {TTL} ms)");
        lapsed.push((which, longest > Duration::from_millis(TTL)));
    }
    assert!(lapsed.iter().all(|(_, l)| !l), "{lapsed:?}");
}
