//! The volume's lease, as the uploader keeps it (M2.4c).
//!
//! The lease is a row of the volume's refs object: a holder, an epoch and
//! an expiry (`skynbd_refs::Lease`). The uploader's commits are fenced by
//! the holder and the epoch (`skynbd_refs::ActingRefStore`), not by the
//! clock: a commit made under an epoch that is no longer the row's lands
//! nothing. The expiry says only how long another holder waits before it
//! may take the lease.
//!
//! The rules, the lead's on the plan of M2.4c:
//!
//! - **The first take** acquires the lease the way any start would: the
//!   row is created if it is absent, renewed if it is this holder's, and
//!   taken with the epoch bumped if another holder's has expired. Another
//!   holder's lease that has not expired is [`LeaseState::Lost`]: at the
//!   start that refuses the serve (M2.4c-2); later, when the start could
//!   not reach the store, it stops the uploads for the rest of the
//!   instance.
//! - **A renewal** (every third of the lease's time, and each commit
//!   renews it too) first reads the row, and takes the lease again only
//!   where the row names this holder at the epoch it last held. Expired or
//!   not: a store out of reach for longer than the lease's time must not
//!   end the uploads for good. Any other holder, or another epoch, is
//!   [`LeaseState::Lost`], and the lease is never taken back from it,
//!   even where its own has expired: that would let a whole cut of this
//!   image go up onto the head of a node that served the volume in
//!   between. A restart is the operator's call.
//! - **A renewal that fails** on the store leaves the lease
//!   [`LeaseState::Unsure`]: the uploads go on, since a commit is fenced
//!   by the epoch, and the next renewal decides.
//! - **A commit refused by the fence** ([`Keeper::fenced`]) is lost too.

use skynbd_refs::{LeaseOutcome, RefStore};
use skynbd_types::VolumeId;

/// Where the lease stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseState {
    /// No lease is held yet, and why: not tried, or the store could not
    /// be reached. Nothing is uploaded.
    None(String),
    /// Held at `epoch`, until `expires_at_ms` by the store's clock unless
    /// renewed.
    Held {
        /// The fence's epoch.
        epoch: u64,
        /// The row's expiry, in Unix milliseconds.
        expires_at_ms: u64,
    },
    /// The last renewal failed on the store, and why. The lease was last
    /// held at `epoch`, and the uploads go on under it: a commit is fenced
    /// by the epoch, so one made after the lease went lands nothing.
    Unsure {
        /// The epoch last held.
        epoch: u64,
        /// The renewal's error.
        why: String,
    },
    /// Lost for good: the row names `holder` at `epoch`. Nothing more is
    /// uploaded by this instance. `holder` is empty where the row has
    /// gone, or where the lease was found taken and given back between
    /// two of this holder's reads.
    Lost {
        /// The holder the row names.
        holder: String,
        /// Its epoch.
        epoch: u64,
    },
}

impl LeaseState {
    /// The epoch the uploads go on under: held, or unsure.
    pub fn epoch(&self) -> Option<u64> {
        match self {
            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => Some(*epoch),
            LeaseState::None(_) | LeaseState::Lost { .. } => None,
        }
    }
}

/// The volume's lease for one holder.
pub struct Keeper<R> {
    refs: R,
    vol: VolumeId,
    holder: String,
    ttl_ms: u64,
    state: LeaseState,
}

impl<R: RefStore> Keeper<R> {
    /// The lease of `vol` in `refs` for `holder`, for `ttl_ms` at each
    /// take. Nothing is read until [`Keeper::keep`].
    pub fn new(refs: R, vol: VolumeId, holder: impl Into<String>, ttl_ms: u64) -> Self {
        Self {
            refs,
            vol,
            holder: holder.into(),
            ttl_ms,
            state: LeaseState::None("not tried yet".into()),
        }
    }

    /// The holder.
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// The lease's time.
    pub fn ttl_ms(&self) -> u64 {
        self.ttl_ms
    }

    /// Where the lease stands.
    pub fn state(&self) -> &LeaseState {
        &self.state
    }

    /// Take the lease, or renew it, by the module's rules. Nothing is done
    /// once it is lost.
    pub async fn keep(&mut self) -> &LeaseState {
        self.state = match &self.state {
            LeaseState::Lost { .. } => return &self.state,
            LeaseState::None(_) => self.take().await,
            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => {
                let epoch = *epoch;
                self.renew(epoch).await
            }
        };
        &self.state
    }

    /// A commit was refused by the fence: the row names `holder` at
    /// `epoch`.
    pub fn fenced(&mut self, holder: String, epoch: u64) {
        self.state = LeaseState::Lost { holder, epoch };
    }

    /// Let the lease go, where it is held (at a clean stop, M2.4c-2), so
    /// that another holder need not wait out its time. The row keeps the
    /// holder and the epoch: the next taker bumps the epoch.
    pub async fn release(&mut self) -> Result<(), String> {
        if self.state.epoch().is_none() {
            return Ok(());
        }
        self.refs
            .lease_release(self.vol, &self.holder)
            .await
            .map_err(|e| e.to_string())?;
        self.state = LeaseState::None("released".into());
        Ok(())
    }

    /// The first take.
    async fn take(&self) -> LeaseState {
        match self
            .refs
            .lease_acquire(self.vol, &self.holder, self.ttl_ms)
            .await
        {
            Ok(LeaseOutcome::Acquired(lease)) => LeaseState::Held {
                epoch: lease.epoch,
                expires_at_ms: lease.expires_at_ms,
            },
            Ok(LeaseOutcome::Held(lease)) => LeaseState::Lost {
                holder: lease.holder,
                epoch: lease.epoch,
            },
            Err(e) => LeaseState::None(e.to_string()),
        }
    }

    /// A renewal of the lease last held at `epoch`.
    async fn renew(&self, epoch: u64) -> LeaseState {
        let unsure = |e: skynbd_refs::RefError| LeaseState::Unsure {
            epoch,
            why: e.to_string(),
        };
        let row = match self.refs.lease_get(self.vol).await {
            Ok(row) => row,
            Err(e) => return unsure(e),
        };
        match row {
            Some(lease) if lease.holder == self.holder && lease.epoch == epoch => {}
            Some(lease) => {
                return LeaseState::Lost {
                    holder: lease.holder,
                    epoch: lease.epoch,
                }
            }
            None => {
                return LeaseState::Lost {
                    holder: String::new(),
                    epoch: 0,
                }
            }
        }
        // The row is this holder's at this epoch: the take keeps the epoch
        // (a holder never fences itself), expired or not.
        match self
            .refs
            .lease_acquire(self.vol, &self.holder, self.ttl_ms)
            .await
        {
            Ok(LeaseOutcome::Acquired(lease)) if lease.epoch == epoch => LeaseState::Held {
                epoch,
                expires_at_ms: lease.expires_at_ms,
            },
            // Another holder took the lease between the read and the take,
            // and its own expired too: this take bumped the epoch. Not
            // this holder's to keep: give it back.
            Ok(LeaseOutcome::Acquired(lease)) => {
                let _ = self.refs.lease_release(self.vol, &self.holder).await;
                LeaseState::Lost {
                    holder: String::new(),
                    epoch: lease.epoch,
                }
            }
            Ok(LeaseOutcome::Held(lease)) => LeaseState::Lost {
                holder: lease.holder,
                epoch: lease.epoch,
            },
            Err(e) => unsure(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use skynbd_refs::ObjectRefStore;
    use skynbd_store::{FaultOp, FaultStore, MemoryStore};

    use super::*;

    const TTL: u64 = 30_000;

    struct World {
        store: FaultStore<MemoryStore>,
        now: Arc<AtomicU64>,
        vol: VolumeId,
    }

    impl World {
        fn new() -> Self {
            Self {
                store: FaultStore::new(MemoryStore::new()),
                now: Arc::new(AtomicU64::new(1_000_000)),
                vol: VolumeId(ulid::Ulid::from(7u128)),
            }
        }

        fn refs(&self) -> ObjectRefStore<FaultStore<MemoryStore>> {
            let now = Arc::clone(&self.now);
            ObjectRefStore::new(self.store.clone())
                .with_clock(Arc::new(move || now.load(Ordering::SeqCst)))
        }

        fn keeper(&self, holder: &str) -> Keeper<ObjectRefStore<FaultStore<MemoryStore>>> {
            Keeper::new(self.refs(), self.vol, holder, TTL)
        }

        fn pass(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }

        async fn row(&self) -> Option<(String, u64)> {
            let lease = self.refs().lease_get(self.vol).await.unwrap()?;
            Some((lease.holder, lease.epoch))
        }

        /// Another holder takes the lease, as it would at a start.
        async fn taken_by(&self, holder: &str) -> LeaseOutcome {
            self.refs()
                .lease_acquire(self.vol, holder, TTL)
                .await
                .unwrap()
        }
    }

    fn held(epoch: u64) -> impl Fn(&LeaseState) -> bool {
        move |s| matches!(s, LeaseState::Held { epoch: e, .. } if *e == epoch)
    }

    #[tokio::test]
    async fn the_first_take_creates_the_row_and_a_renewal_keeps_its_epoch() {
        let w = World::new();
        let mut k = w.keeper("a");
        assert_eq!(k.state(), &LeaseState::None("not tried yet".into()));
        assert!(held(1)(k.keep().await));
        assert_eq!(w.row().await, Some(("a".into(), 1)));
        let LeaseState::Held { expires_at_ms, .. } = k.state().clone() else {
            panic!("held")
        };
        w.pass(10_000);
        let LeaseState::Held {
            epoch,
            expires_at_ms: later,
        } = k.keep().await.clone()
        else {
            panic!("held")
        };
        assert_eq!(epoch, 1);
        assert_eq!(later, expires_at_ms + 10_000);
    }

    #[tokio::test]
    async fn another_holders_lease_at_the_first_take_is_lost_and_left_alone() {
        let w = World::new();
        assert!(matches!(w.taken_by("b").await, LeaseOutcome::Acquired(_)));
        let mut k = w.keeper("a");
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "b".into(),
                epoch: 1
            }
        );
        assert_eq!(w.row().await, Some(("b".into(), 1)));
        // Lost is for good: nothing more is read or taken, even once the
        // other's lease has expired.
        w.pass(TTL * 2);
        assert!(matches!(k.keep().await, LeaseState::Lost { .. }));
        assert_eq!(w.row().await, Some(("b".into(), 1)));
    }

    #[tokio::test]
    async fn an_expired_lease_of_another_holder_is_taken_at_the_first_take() {
        let w = World::new();
        w.taken_by("b").await;
        w.pass(TTL + 1);
        let mut k = w.keeper("a");
        assert!(held(2)(k.keep().await));
        assert_eq!(w.row().await, Some(("a".into(), 2)));
    }

    #[tokio::test]
    async fn a_store_out_of_reach_at_the_first_take_leaves_no_lease_and_a_later_take_holds() {
        let w = World::new();
        w.store.arm(FaultOp::Any, "", 0).await;
        let mut k = w.keeper("a");
        assert!(matches!(k.keep().await, LeaseState::None(why) if why.contains("injected")));
        assert_eq!(k.state().epoch(), None);
        w.store.disarm().await;
        assert!(held(1)(k.keep().await));
    }

    #[tokio::test]
    async fn an_outage_longer_than_the_lease_does_not_lose_it_where_nobody_else_took_it() {
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        w.store.arm(FaultOp::Any, "", 0).await;
        assert!(matches!(
            k.keep().await,
            LeaseState::Unsure { epoch: 1, .. }
        ));
        assert_eq!(k.state().epoch(), Some(1));
        w.pass(TTL * 4);
        assert!(matches!(
            k.keep().await,
            LeaseState::Unsure { epoch: 1, .. }
        ));
        w.store.disarm().await;
        assert!(held(1)(k.keep().await));
        assert_eq!(w.row().await, Some(("a".into(), 1)));
    }

    #[tokio::test]
    async fn a_lease_another_holder_took_during_an_outage_is_lost_and_never_taken_back() {
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        w.store.arm(FaultOp::Any, "", 0).await;
        k.keep().await;
        w.pass(TTL + 1);
        w.store.disarm().await;
        assert!(matches!(w.taken_by("b").await, LeaseOutcome::Acquired(_)));
        // The other holder's lease expires too before this one's renewal:
        // a take would steal it. The renewal does not take.
        w.pass(TTL + 1);
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "b".into(),
                epoch: 2
            }
        );
        assert_eq!(w.row().await, Some(("b".into(), 2)));
    }

    #[tokio::test]
    async fn this_holders_name_at_another_epoch_is_lost() {
        // Another instance under the same name took the lease after a
        // third holder had it: the epoch says it is not this one's.
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        w.pass(TTL + 1);
        w.taken_by("b").await;
        w.pass(TTL + 1);
        assert!(matches!(w.taken_by("a").await, LeaseOutcome::Acquired(l) if l.epoch == 3));
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "a".into(),
                epoch: 3
            }
        );
        assert_eq!(w.row().await, Some(("a".into(), 3)));
    }

    #[tokio::test]
    async fn a_fenced_commit_loses_the_lease_and_a_release_lets_a_held_one_go() {
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        k.release().await.unwrap();
        assert_eq!(k.state(), &LeaseState::None("released".into()));
        // Released: another holder takes it at once, with the epoch bumped.
        assert!(matches!(w.taken_by("b").await, LeaseOutcome::Acquired(l) if l.epoch == 2));
        let mut k = w.keeper("c");
        k.fenced("b".into(), 2);
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "b".into(),
                epoch: 2
            }
        );
        // Nothing held: nothing to release.
        k.release().await.unwrap();
        assert_eq!(w.row().await, Some(("b".into(), 2)));
    }

    /// The refs object, where another holder takes the lease between
    /// this holder's read of the row and its take, and the clock then
    /// moves on by `jump` (past the other's expiry, or not): the race the
    /// renewal's second check is for. With `read_as`, the read names that
    /// holder in place of the row's: a row deleted and taken anew under
    /// another name at the same epoch, which the refs store's own calls
    /// cannot make.
    struct Racing {
        inner: ObjectRefStore<FaultStore<MemoryStore>>,
        other: ObjectRefStore<FaultStore<MemoryStore>>,
        now: Arc<AtomicU64>,
        armed: std::sync::atomic::AtomicBool,
        jump: u64,
        read_as: Option<&'static str>,
    }

    impl Racing {
        fn new(w: &World, jump: u64, read_as: Option<&'static str>) -> Self {
            Self {
                inner: w.refs(),
                other: w.refs(),
                now: Arc::clone(&w.now),
                armed: std::sync::atomic::AtomicBool::new(false),
                jump,
                read_as,
            }
        }
    }

    #[async_trait::async_trait]
    impl RefStore for Racing {
        async fn get(
            &self,
            vol: VolumeId,
            name: &str,
        ) -> skynbd_refs::Result<Option<skynbd_refs::RefValue>> {
            self.inner.get(vol, name).await
        }
        async fn compare_and_swap(
            &self,
            vol: VolumeId,
            name: &str,
            expected: Option<skynbd_refs::RefValue>,
            new_commit: Option<skynbd_types::CommitHash>,
        ) -> skynbd_refs::Result<skynbd_refs::CasOutcome> {
            self.inner
                .compare_and_swap(vol, name, expected, new_commit)
                .await
        }
        async fn list(
            &self,
            vol: VolumeId,
        ) -> skynbd_refs::Result<Vec<(String, skynbd_refs::RefValue)>> {
            self.inner.list(vol).await
        }
        fn watch(&self) -> tokio::sync::broadcast::Receiver<skynbd_refs::RefEvent> {
            self.inner.watch()
        }
        async fn transact(
            &self,
            vol: VolumeId,
            ops: Vec<skynbd_refs::RefOp>,
        ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
            self.inner.transact(vol, ops).await
        }
        async fn transact_as(
            &self,
            vol: VolumeId,
            ops: Vec<skynbd_refs::RefOp>,
            holder: &str,
            epoch: u64,
        ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
            self.inner.transact_as(vol, ops, holder, epoch).await
        }
        async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
            self.inner.pins(vol, name).await
        }
        async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
            self.inner.meta_get(vol, key).await
        }
        async fn meta_list(
            &self,
            vol: VolumeId,
            prefix: &str,
        ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
            self.inner.meta_list(vol, prefix).await
        }
        async fn lease_acquire(
            &self,
            vol: VolumeId,
            holder: &str,
            ttl_ms: u64,
        ) -> skynbd_refs::Result<LeaseOutcome> {
            self.inner.lease_acquire(vol, holder, ttl_ms).await
        }
        async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
            self.inner.lease_release(vol, holder).await
        }
        async fn lease_get(
            &self,
            vol: VolumeId,
        ) -> skynbd_refs::Result<Option<skynbd_refs::Lease>> {
            let row = self.inner.lease_get(vol).await;
            if self.armed.swap(false, Ordering::SeqCst) {
                let taken = self.other.lease_acquire(vol, "b", TTL).await.unwrap();
                assert!(matches!(taken, LeaseOutcome::Acquired(_)), "{taken:?}");
                self.now.fetch_add(self.jump, Ordering::SeqCst);
            }
            match (row, self.read_as) {
                (Ok(Some(lease)), Some(holder)) => Ok(Some(skynbd_refs::Lease {
                    holder: holder.into(),
                    ..lease
                })),
                (row, _) => row,
            }
        }
    }

    /// The row is this holder's at its epoch when read, but another holder
    /// takes the lease before this holder's take and lets it expire: the
    /// take steals it back at a newer epoch. That epoch is not the one
    /// the uploads are fenced by, so the lease is given back and lost.
    #[tokio::test]
    async fn a_lease_taken_and_let_go_by_another_between_the_read_and_the_take_is_lost() {
        let w = World::new();
        let mut k = Keeper::new(Racing::new(&w, 2 * TTL, None), w.vol, "a", TTL);
        assert!(held(1)(k.keep().await));
        // This holder's lease runs out (an outage), and the race is armed.
        w.pass(2 * TTL);
        k.refs.armed.store(true, Ordering::SeqCst);
        let state = k.keep().await.clone();
        assert_eq!(
            state,
            LeaseState::Lost {
                holder: String::new(),
                epoch: 3
            }
        );
        // Given back: the row names this holder at the newer epoch, released.
        let row = w.refs().lease_get(w.vol).await.unwrap().expect("the row");
        assert_eq!(
            (row.holder.as_str(), row.epoch, row.expires_at_ms),
            ("a", 3, 0)
        );
        // And never taken again.
        assert_eq!(k.keep().await.clone(), state);
        assert_eq!(w.row().await, Some(("a".into(), 3)));
    }

    /// The race again, with the other holder's lease still live at this
    /// holder's take (K5 of #117's review): the take finds it held, and
    /// the lease is lost to that holder, not unsure.
    #[tokio::test]
    async fn a_lease_another_holder_took_between_the_read_and_the_take_is_lost_to_it() {
        let w = World::new();
        let mut k = Keeper::new(Racing::new(&w, 0, None), w.vol, "a", TTL);
        assert!(held(1)(k.keep().await));
        w.pass(2 * TTL);
        k.refs.armed.store(true, Ordering::SeqCst);
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "b".into(),
                epoch: 2
            }
        );
        assert_eq!(w.row().await, Some(("b".into(), 2)));
    }

    /// A row that names another holder at this holder's epoch is not this
    /// holder's (K2 of #117's review): the renewal compares the holder as
    /// well as the epoch, and takes nothing.
    #[tokio::test]
    async fn a_row_of_another_name_at_this_holders_epoch_is_lost() {
        let w = World::new();
        let mut k = Keeper::new(Racing::new(&w, 0, Some("b")), w.vol, "a", TTL);
        assert!(held(1)(k.keep().await));
        let LeaseState::Held { expires_at_ms, .. } = k.state().clone() else {
            panic!("held")
        };
        w.pass(TTL / 3);
        assert_eq!(
            k.keep().await,
            &LeaseState::Lost {
                holder: "b".into(),
                epoch: 1
            }
        );
        let row = w.refs().lease_get(w.vol).await.unwrap().expect("the row");
        assert_eq!(row.expires_at_ms, expires_at_ms, "nothing taken");
    }

    /// A commit the fence refused loses a held lease at once, naming the
    /// row's holder, and nothing is taken after (K8 of #117's review).
    #[tokio::test]
    async fn a_fenced_commit_loses_a_held_lease_at_once() {
        let w = World::new();
        let mut k = w.keeper("a");
        assert!(held(1)(k.keep().await));
        k.fenced("b".into(), 2);
        let lost = LeaseState::Lost {
            holder: "b".into(),
            epoch: 2,
        };
        assert_eq!(k.state(), &lost);
        assert_eq!(k.state().epoch(), None, "no epoch to upload under");
        w.pass(TTL / 3);
        assert_eq!(k.keep().await, &lost);
        assert_eq!(w.row().await, Some(("a".into(), 1)), "nothing taken");
    }

    /// An unsure lease is let go at a release as a held one is (K9 of
    /// #117's review): the row may still be this holder's.
    #[tokio::test]
    async fn an_unsure_lease_is_let_go_at_a_release() {
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        w.store.arm(FaultOp::Any, "", 0).await;
        assert!(matches!(
            k.keep().await,
            LeaseState::Unsure { epoch: 1, .. }
        ));
        w.store.disarm().await;
        k.release().await.unwrap();
        assert_eq!(k.state(), &LeaseState::None("released".into()));
        let row = w.refs().lease_get(w.vol).await.unwrap().expect("the row");
        assert_eq!(
            (row.holder.as_str(), row.epoch, row.expires_at_ms),
            ("a", 1, 0)
        );
    }

    /// A renewal whose read finds the row this holder's and whose take
    /// then fails on the store leaves the lease unsure, not lost (K13 of
    /// #117's review): nothing says that anyone else holds it.
    #[tokio::test]
    async fn a_renewal_whose_take_fails_after_its_read_is_unsure() {
        let w = World::new();
        let mut k = w.keeper("a");
        k.keep().await;
        w.store.arm(FaultOp::Put, "", 0).await;
        assert!(matches!(
            k.keep().await,
            LeaseState::Unsure { epoch: 1, .. }
        ));
        w.store.disarm().await;
        assert!(held(1)(k.keep().await));
    }
}
