//! Review scenarios for #131 (F5), copied into crates/skynbd-refs/tests/
//! as review_f5.rs for a run and removed after. Not part of the pull request.
//!
//! E1: every sequence of up to three other operations put between the
//!     renewal's read of the refs object and its conditional write, from
//!     seven starting rows, on the real ObjectRefStore over three object
//!     stores: MemoryStore (a version counter as the etag), a store whose
//!     etag is a hash of the content (as S3's is), and, where
//!     REVIEW_S3_ENDPOINT is set, a live S3 endpoint. Each result is
//!     compared with an independent model of D-M3's rules taken one of
//!     the two ways the renewal may be ordered with the others: after
//!     them, or (for an answer of NotHeld, which writes nothing) before.
//! E2: random sequences of acquire, renew, release, commit and clock
//!     moves, the same sequence on the memory, redb, object and cadenced
//!     stores and on the model; every answer and every row compared.
//! E3: the F5 race on the object store with the other holder's take put
//!     between the read and the write, where the PUT is then refused and
//!     the store must judge again: the renewal's own decision is counted.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use skynbd_refs::{
    refs_key, CadencedRefStore, Clock, Lease, LeaseOutcome, MemoryRefStore, ObjectRefStore,
    RedbRefStore, RefError, RefOp, RefStore, RenewOutcome, TxnOutcome,
};
use skynbd_store::{MemoryStore, ObjectStore, Precondition, StoreError};
use skynbd_types::VolumeId;

const TTL: u64 = 10_000;
const T0: u64 = 1_000_000;

fn vol() -> VolumeId {
    VolumeId(ulid::Ulid::from_parts(0xF5, 1))
}

fn clock_of(now: &Arc<AtomicU64>) -> Clock {
    let n = Arc::clone(now);
    Arc::new(move || n.load(Ordering::SeqCst))
}

// ---------------------------------------------------------------------
// The model: D-M3 as the docs state it, written apart from the crate.
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    holder: String,
    epoch: u64,
    exp: u64,
}

fn row_of(l: &Lease) -> Row {
    Row {
        holder: l.holder.clone(),
        epoch: l.epoch,
        exp: l.expires_at_ms,
    }
}

#[derive(Clone, Debug, Default)]
struct Model {
    row: Option<Row>,
    now: u64,
}

impl Model {
    fn new() -> Self {
        Model { row: None, now: T0 }
    }
    fn acquire(&mut self, h: &str) -> String {
        let fresh = |epoch| Row {
            holder: h.into(),
            epoch,
            exp: self.now + TTL,
        };
        let (row, out) = match &self.row {
            None => (Some(fresh(1)), None),
            Some(r) if r.holder == h => (Some(fresh(r.epoch)), None),
            Some(r) if r.exp <= self.now => (Some(fresh(r.epoch + 1)), None),
            Some(r) => (None, Some(format!("Held({r:?})"))),
        };
        match (row, out) {
            (Some(r), _) => {
                self.row = Some(r.clone());
                format!("Acquired({r:?})")
            }
            (None, Some(o)) => o,
            _ => unreachable!(),
        }
    }
    fn renew(&mut self, h: &str, e: u64) -> String {
        match &self.row {
            Some(r) if r.holder == h && r.epoch == e => {
                let r = Row {
                    holder: h.into(),
                    epoch: e,
                    exp: self.now + TTL,
                };
                self.row = Some(r.clone());
                format!("Renewed({r:?})")
            }
            other => format!("NotHeld({other:?})"),
        }
    }
    fn release(&mut self, h: &str) -> String {
        if let Some(r) = &mut self.row {
            if r.holder == h {
                r.exp = 0;
            }
        }
        "released".into()
    }
    fn commit(&mut self, h: &str, e: u64) -> String {
        match &self.row {
            Some(r) if r.holder == h && r.epoch == e => "Committed".into(),
            Some(r) => format!("LeaseLost({}, {})", r.holder, r.epoch),
            None => "LeaseLost(, 0)".into(),
        }
    }
}

fn show_acquire(o: &LeaseOutcome) -> String {
    match o {
        LeaseOutcome::Acquired(l) => format!("Acquired({:?})", row_of(l)),
        LeaseOutcome::Held(l) => format!("Held({:?})", row_of(l)),
    }
}

fn show_renew(o: &RenewOutcome) -> String {
    match o {
        RenewOutcome::Renewed(l) => format!("Renewed({:?})", row_of(l)),
        RenewOutcome::NotHeld(r) => format!("NotHeld({:?})", r.as_ref().map(row_of)),
    }
}

// ---------------------------------------------------------------------
// Operations, on a store and on the model
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Acq(&'static str),
    /// A renewal at a fixed epoch.
    Ren(&'static str, u64),
    /// A renewal by the holder's keeper: at the row's epoch where the row
    /// is its own, else at 1.
    RenCur(&'static str),
    Rel(&'static str),
    Tick(u64),
    /// The refs object deleted (object stores only).
    Del,
    /// A commit fenced as the holder at the row's epoch where the row is
    /// its own, else at 1.
    Commit(&'static str),
}

fn cur_epoch(row: Option<&Row>, h: &str) -> u64 {
    match row {
        Some(r) if r.holder == h => r.epoch,
        _ => 1,
    }
}

impl Model {
    fn apply(&mut self, op: Op) -> String {
        match op {
            Op::Acq(h) => self.acquire(h),
            Op::Ren(h, e) => self.renew(h, e),
            Op::RenCur(h) => {
                let e = cur_epoch(self.row.as_ref(), h);
                self.renew(h, e)
            }
            Op::Rel(h) => self.release(h),
            Op::Tick(ms) => {
                self.now += ms;
                "tick".into()
            }
            Op::Del => {
                self.row = None;
                "deleted".into()
            }
            Op::Commit(h) => {
                let e = cur_epoch(self.row.as_ref(), h);
                self.commit(h, e)
            }
        }
    }
}

fn show_err(e: RefError) -> String {
    match e {
        RefError::LeaseLost { holder, epoch, .. } => format!("LeaseLost({holder}, {epoch})"),
        other => format!("Err({other})"),
    }
}

static COMMITS: AtomicU64 = AtomicU64::new(0);

async fn apply(
    s: &dyn RefStore,
    op: Op,
    now: &AtomicU64,
    objects: Option<&dyn ObjectStore>,
) -> String {
    let v = vol();
    match op {
        Op::Acq(h) => match s.lease_acquire(v, h, TTL).await {
            Ok(o) => show_acquire(&o),
            Err(e) => show_err(e),
        },
        Op::Ren(h, e) => match s.lease_renew(v, h, e, TTL).await {
            Ok(o) => show_renew(&o),
            Err(e) => show_err(e),
        },
        Op::RenCur(h) => {
            let row = s.lease_get(v).await.expect("a read").map(|l| row_of(&l));
            let e = cur_epoch(row.as_ref(), h);
            match s.lease_renew(v, h, e, TTL).await {
                Ok(o) => show_renew(&o),
                Err(e) => show_err(e),
            }
        }
        Op::Rel(h) => match s.lease_release(v, h).await {
            Ok(()) => "released".into(),
            Err(e) => show_err(e),
        },
        Op::Tick(ms) => {
            now.fetch_add(ms, Ordering::SeqCst);
            "tick".into()
        }
        Op::Del => {
            objects
                .expect("an object store")
                .delete(&refs_key(v))
                .await
                .expect("deleted");
            "deleted".into()
        }
        Op::Commit(h) => {
            let row = s.lease_get(v).await.expect("a read").map(|l| row_of(&l));
            let e = cur_epoch(row.as_ref(), h);
            let n = COMMITS.fetch_add(1, Ordering::SeqCst);
            let op = RefOp::MetaPut {
                key: format!("review/{n}"),
                value: vec![1],
            };
            match s.transact_as(v, vec![op], h, e).await {
                Ok(TxnOutcome::Committed) => "Committed".into(),
                Ok(other) => format!("{other:?}"),
                Err(e) => show_err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------
// Object stores: one whose etag is a hash of the content, as S3's is
// ---------------------------------------------------------------------

#[derive(Default)]
struct ContentEtag {
    map: Mutex<BTreeMap<String, Bytes>>,
}

fn etag_of(b: &Bytes) -> String {
    let mut h = DefaultHasher::new();
    b.hash(&mut h);
    format!("{:016x}-{}", h.finish(), b.len())
}

#[async_trait::async_trait]
impl ObjectStore for ContentEtag {
    async fn put(&self, key: &str, data: Bytes) -> skynbd_store::Result<()> {
        self.map.lock().unwrap().insert(key.into(), data);
        Ok(())
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> skynbd_store::Result<Bytes> {
        let m = self.map.lock().unwrap();
        let b = m.get(key).ok_or_else(|| StoreError::NotFound(key.into()))?;
        Ok(match range {
            None => b.clone(),
            Some(r) => b.slice(r.start as usize..r.end as usize),
        })
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        Ok(self.map.lock().unwrap().contains_key(key))
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
    async fn get_versioned(&self, key: &str) -> skynbd_store::Result<(Bytes, String)> {
        let m = self.map.lock().unwrap();
        let b = m.get(key).ok_or_else(|| StoreError::NotFound(key.into()))?;
        Ok((b.clone(), etag_of(b)))
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: Precondition,
    ) -> skynbd_store::Result<String> {
        let mut m = self.map.lock().unwrap();
        let ok = match (&pre, m.get(key)) {
            (Precondition::IfNoneMatch, None) => true,
            (Precondition::IfMatch(e), Some(b)) => *e == etag_of(b),
            _ => false,
        };
        if !ok {
            return Err(StoreError::PreconditionFailed(key.into()));
        }
        let e = etag_of(&data);
        m.insert(key.into(), data);
        Ok(e)
    }
}

/// The renewal's way to the bucket: after its first read of the refs
/// object, the armed operations run (on another store over the same
/// objects), and then the read is answered. Counts the conditional
/// writes and how many were refused.
struct Inject {
    shared: Arc<dyn ObjectStore>,
    other: ObjectRefStore<Arc<dyn ObjectStore>>,
    now: Arc<AtomicU64>,
    armed: Mutex<Option<Vec<Op>>>,
    seen: Mutex<Vec<String>>,
    writes: AtomicUsize,
    refused: AtomicUsize,
}

#[async_trait::async_trait]
impl ObjectStore for Inject {
    async fn put(&self, key: &str, data: Bytes) -> skynbd_store::Result<()> {
        self.shared.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> skynbd_store::Result<Bytes> {
        self.shared.get(key, range).await
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        self.shared.exists(key).await
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        self.shared.list(prefix).await
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.shared.delete(key).await
    }
    async fn get_versioned(&self, key: &str) -> skynbd_store::Result<(Bytes, String)> {
        let read = self.shared.get_versioned(key).await;
        let ops = self.armed.lock().unwrap().take();
        if let Some(ops) = ops {
            for op in ops {
                let out = apply(&self.other, op, &self.now, Some(&*self.shared)).await;
                self.seen.lock().unwrap().push(out);
            }
        }
        read
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: Precondition,
    ) -> skynbd_store::Result<String> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let r = self.shared.put_if(key, data, pre).await;
        if matches!(r, Err(StoreError::PreconditionFailed(_))) {
            self.refused.fetch_add(1, Ordering::SeqCst);
        }
        r
    }
}

const ALPHABET: [Op; 9] = [
    Op::Acq("b"),
    Op::Acq("a"),
    Op::Rel("a"),
    Op::Rel("b"),
    Op::RenCur("a"),
    Op::RenCur("b"),
    Op::Tick(TTL + 1),
    Op::Del,
    Op::Commit("b"),
];

fn starts() -> Vec<(&'static str, Vec<Op>, u64)> {
    use Op::*;
    vec![
        ("no row", vec![], 1),
        ("a at 1, live", vec![Acq("a")], 1),
        ("a at 1, expired", vec![Acq("a"), Tick(TTL + 1)], 1),
        ("a at 1, released", vec![Acq("a"), Rel("a")], 1),
        ("b at 2, live", vec![Acq("a"), Tick(TTL + 1), Acq("b")], 1),
        (
            "b at 2, expired",
            vec![Acq("a"), Tick(TTL + 1), Acq("b"), Tick(TTL + 1)],
            1,
        ),
        (
            "a at 3, live",
            vec![Acq("a"), Tick(TTL + 1), Acq("b"), Rel("b"), Acq("a")],
            3,
        ),
        (
            "a at 3, live, a stale renewal at 1",
            vec![Acq("a"), Tick(TTL + 1), Acq("b"), Rel("b"), Acq("a")],
            1,
        ),
    ]
}

fn sequences(max: usize) -> Vec<Vec<Op>> {
    let mut all = vec![vec![]];
    let mut last = vec![vec![]];
    for _ in 0..max {
        let mut next = Vec::new();
        for s in &last {
            for op in ALPHABET {
                let mut t = s.clone();
                t.push(op);
                next.push(t);
            }
        }
        all.extend(next.iter().cloned());
        last = next;
    }
    all
}

#[derive(Default, Debug)]
struct Tally {
    cases: usize,
    after: usize,
    before: usize,
    renewed: usize,
    not_held: usize,
    errors: usize,
    refused_then_judged: usize,
    mismatches: Vec<String>,
    /// Mismatches whose operations in between include no delete.
    without_delete: usize,
}

/// One case of E1 on the objects `shared` (empty at the start).
async fn e1_case(
    shared: Arc<dyn ObjectStore>,
    prefix: &[Op],
    epoch: u64,
    inj: &[Op],
    tally: &mut Tally,
    label: &str,
) {
    let now = Arc::new(AtomicU64::new(T0));
    let clock = clock_of(&now);
    let plain = ObjectRefStore::new(Arc::clone(&shared)).with_clock(Arc::clone(&clock));
    let mut model = Model::new();
    for op in prefix {
        let real = apply(&plain, *op, &now, Some(&*shared)).await;
        let want = model.apply(*op);
        assert_eq!(real, want, "{label}: the prefix {prefix:?} at {op:?}");
    }
    let inject = Arc::new(Inject {
        shared: Arc::clone(&shared),
        other: ObjectRefStore::new(Arc::clone(&shared)).with_clock(Arc::clone(&clock)),
        now: Arc::clone(&now),
        armed: Mutex::new(Some(inj.to_vec())),
        seen: Mutex::new(Vec::new()),
        writes: AtomicUsize::new(0),
        refused: AtomicUsize::new(0),
    });
    let a = ObjectRefStore::new(Arc::clone(&inject) as Arc<dyn ObjectStore>)
        .with_clock(Arc::clone(&clock));
    let real = match a.lease_renew(vol(), "a", epoch, TTL).await {
        Ok(o) => show_renew(&o),
        Err(e) => show_err(e),
    };
    let real_row = plain.lease_get(vol()).await.expect("a read").map(|l| row_of(&l));
    let seen = inject.seen.lock().unwrap().clone();

    // After: the others, then the renewal.
    let mut after = model.clone();
    let after_seen: Vec<String> = inj.iter().map(|op| after.apply(*op)).collect();
    let after_out = after.renew("a", epoch);
    // Before: the renewal (NotHeld writes nothing), then the others.
    let mut before = model.clone();
    let before_out = before.renew("a", epoch);
    let before_seen: Vec<String> = inj.iter().map(|op| before.apply(*op)).collect();

    tally.cases += 1;
    if real.starts_with("Renewed") {
        tally.renewed += 1;
    } else if real.starts_with("NotHeld") {
        tally.not_held += 1;
    } else {
        tally.errors += 1;
    }
    if inject.refused.load(Ordering::SeqCst) > 0 {
        tally.refused_then_judged += 1;
    }
    if real == after_out && real_row == after.row && seen == after_seen {
        tally.after += 1;
    } else if real.starts_with("NotHeld")
        && real == before_out
        && real_row == before.row
        && seen == before_seen
    {
        tally.before += 1;
    } else if !inj.contains(&Op::Del) && !prefix.contains(&Op::Del) {
        tally.without_delete += 1;
        tally.mismatches.push(format!("NO DELETE: {label}: {prefix:?} {epoch} {inj:?}: {real} {real_row:?}"));
    } else if tally.mismatches.len() < 12 {
        tally.mismatches.push(format!(
            "{label}: start {prefix:?}, renew a at {epoch}, between read and write {inj:?}\n    \
             got {real} row {real_row:?} others {seen:?}\n    \
             after-order {after_out} row {:?} others {after_seen:?}\n    \
             before-order {before_out} row {:?}; writes {} refused {}",
            after.row,
            before.row,
            inject.writes.load(Ordering::SeqCst),
            inject.refused.load(Ordering::SeqCst),
        ));
    } else {
        tally.mismatches.push(String::new());
    }
}

async fn e1_on(label: &str, make: impl Fn() -> Arc<dyn ObjectStore>, max: usize) -> Tally {
    let mut tally = Tally::default();
    for (_, prefix, epoch) in starts() {
        for inj in sequences(max) {
            e1_case(make(), &prefix, epoch, &inj, &mut tally, label).await;
        }
    }
    println!(
        "## E1 {label}: {} cases; matched the renewal after the others {}, before them {}; \
         renewed {}, not held {}, errors {}; a write refused and judged again in {}; \
         mismatches {} ({} without a delete)",
        tally.cases,
        tally.after,
        tally.before,
        tally.renewed,
        tally.not_held,
        tally.errors,
        tally.refused_then_judged,
        tally.mismatches.len(),
        tally.without_delete
    );
    for m in tally.mismatches.iter().filter(|m| !m.is_empty()) {
        println!("  {m}");
    }
    tally
}

#[tokio::test]
async fn e1_every_interleaving_between_the_renewals_read_and_write_content_etag() {
    let t = e1_on(
        "content etag (as S3)",
        || Arc::new(ContentEtag::default()) as Arc<dyn ObjectStore>,
        3,
    )
    .await;
    assert!(t.mismatches.is_empty());
}

#[tokio::test]
async fn e1_every_interleaving_between_the_renewals_read_and_write_memory_store() {
    // Records: MemoryStore's version counter restarts at 0 when the
    // object is deleted, so an etag can come back after a delete.
    let _ = e1_on(
        "MemoryStore",
        || Arc::new(MemoryStore::new()) as Arc<dyn ObjectStore>,
        3,
    )
    .await;
}

#[tokio::test]
async fn e1_without_deletes_memory_store() {
    // The same without the delete in the alphabet: every case matches.
    let mut tally = Tally::default();
    for (_, prefix, epoch) in starts() {
        for inj in sequences(3).into_iter().filter(|s| !s.contains(&Op::Del)) {
            e1_case(
                Arc::new(MemoryStore::new()),
                &prefix,
                epoch,
                &inj,
                &mut tally,
                "MemoryStore, no delete",
            )
            .await;
        }
    }
    println!(
        "## E1 MemoryStore without deletes: {} cases; after {}, before {}; renewed {}, \
         not held {}, errors {}; refused and judged again {}; mismatches {}",
        tally.cases,
        tally.after,
        tally.before,
        tally.renewed,
        tally.not_held,
        tally.errors,
        tally.refused_then_judged,
        tally.mismatches.len()
    );
    for m in tally.mismatches.iter().filter(|m| !m.is_empty()) {
        println!("  {m}");
    }
    assert!(tally.mismatches.is_empty());
}

#[tokio::test]
async fn e1_live_s3() {
    let Ok(endpoint) = std::env::var("REVIEW_S3_ENDPOINT") else {
        println!("## E1 live S3: skipped, REVIEW_S3_ENDPOINT not set");
        return;
    };
    // A bucket of its own for each run.
    let bucket = format!("review-f5-{}", ulid::Ulid::new().to_string().to_lowercase());
    let store = skynbd_store::S3Store::new(&endpoint, &bucket, "test", "test", "us-east-1", true)
        .await;
    store.ensure_bucket().await.expect("the bucket");
    let store: Arc<dyn ObjectStore> = Arc::new(store);
    ObjectRefStore::probe(&store).await.expect("conditional writes");
    let mut tally = Tally::default();
    let mut n = 0u128;
    for (_, prefix, epoch) in starts() {
        for inj in sequences(2) {
            n += 1;
            // A volume of its own for each case, on the same bucket.
            let shared: Arc<dyn ObjectStore> = Arc::new(Prefixed {
                inner: Arc::clone(&store),
                n,
            });
            e1_case(shared, &prefix, epoch, &inj, &mut tally, "live S3").await;
        }
    }
    println!(
        "## E1 live S3: {} cases; after {}, before {}; renewed {}, not held {}, errors {}; \
         refused and judged again {}; mismatches {} ({} without a delete)",
        tally.cases,
        tally.after,
        tally.before,
        tally.renewed,
        tally.not_held,
        tally.errors,
        tally.refused_then_judged,
        tally.mismatches.len(),
        tally.without_delete
    );
    for m in tally.mismatches.iter().filter(|m| !m.is_empty()) {
        println!("  {m}");
    }
    // A delete between the read and the write: S3 answers 404 to the
    // conditional PUT, which the store gives as an error (recorded).
    assert_eq!(tally.without_delete, 0);
}

/// Keys of one case put under a prefix of their own on a shared bucket.
struct Prefixed {
    inner: Arc<dyn ObjectStore>,
    n: u128,
}

impl Prefixed {
    fn k(&self, key: &str) -> String {
        format!("case-{}/{key}", self.n)
    }
}

#[async_trait::async_trait]
impl ObjectStore for Prefixed {
    async fn put(&self, key: &str, data: Bytes) -> skynbd_store::Result<()> {
        self.inner.put(&self.k(key), data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> skynbd_store::Result<Bytes> {
        self.inner.get(&self.k(key), range).await
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        self.inner.exists(&self.k(key)).await
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        self.inner.list(&self.k(prefix)).await
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.inner.delete(&self.k(key)).await
    }
    async fn get_versioned(&self, key: &str) -> skynbd_store::Result<(Bytes, String)> {
        self.inner.get_versioned(&self.k(key)).await
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: Precondition,
    ) -> skynbd_store::Result<String> {
        self.inner.put_if(&self.k(key), data, pre).await
    }
}

// ---------------------------------------------------------------------
// E2: one sequence on every store and on the model
// ---------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_op(r: &mut Rng) -> Op {
    let h = ["a", "b", "c"][r.below(3) as usize];
    match r.below(8) {
        0 | 1 => Op::Acq(h),
        2 => Op::Ren(h, r.below(5)),
        3 => Op::RenCur(h),
        4 => Op::Rel(h),
        5 => Op::Tick(r.below(2 * TTL)),
        6 => Op::Tick(TTL + 1),
        _ => Op::Commit(h),
    }
}

#[tokio::test]
async fn e2_one_sequence_on_every_store_and_the_model() {
    let dir = tempfile::tempdir().expect("a directory");
    let (mut ops, mut renews, mut renewed) = (0usize, 0usize, 0usize);
    for seed in 1..=600u64 {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let now = Arc::new(AtomicU64::new(T0));
        let clock = clock_of(&now);
        let memory = MemoryRefStore::new().with_clock(Arc::clone(&clock));
        let redb = RedbRefStore::open(dir.path().join(format!("{seed}.redb")))
            .expect("redb")
            .with_clock(Arc::clone(&clock));
        let object = ObjectRefStore::new(MemoryStore::new()).with_clock(Arc::clone(&clock));
        let cadenced = CadencedRefStore::new(
            ObjectRefStore::new(MemoryStore::new()).with_clock(Arc::clone(&clock)),
        );
        let stores: [(&str, &dyn RefStore); 4] = [
            ("memory", &memory),
            ("redb", &redb),
            ("object", &object),
            ("cadenced", &cadenced),
        ];
        let mut model = Model::new();
        let mut trace = Vec::new();
        for _ in 0..40 {
            let op = random_op(&mut r);
            trace.push(op);
            ops += 1;
            let want = model.apply(op);
            if matches!(op, Op::Ren(..) | Op::RenCur(_)) {
                renews += 1;
                if want.starts_with("Renewed") {
                    renewed += 1;
                }
            }
            // A tick moves the one clock all the stores read.
            if let Op::Tick(_) = op {
                apply(&memory, op, &now, None).await;
                continue;
            }
            for (name, s) in stores {
                let got = apply(s, op, &now, None).await;
                assert_eq!(got, want, "seed {seed}, {name}, {trace:?}");
                let row = s.lease_get(vol()).await.expect("a read").map(|l| row_of(&l));
                assert_eq!(row, model.row, "seed {seed}, {name}, row after {trace:?}");
            }
        }
    }
    println!(
        "## E2: 600 sequences, {ops} operations on four stores and the model; \
         {renews} renewals, {renewed} renewed; no difference"
    );
}

// ---------------------------------------------------------------------
// E3: the take between the read and the write, counted
// ---------------------------------------------------------------------

/// The F5 race on the object store, and the same race where the other
/// holder releases at once (a released row of another holder is not
/// this holder's), and where a second daemon of this holder's name takes
/// it again (at a newer epoch: not this keeper's).
#[tokio::test]
async fn e3_races_at_the_write() {
    use Op::*;
    let cases: Vec<(&str, Vec<Op>, Vec<Op>, u64, &str)> = vec![
        (
            "F5: b takes the expired lease and lets it run out",
            vec![Acq("a"), Tick(TTL + 1)],
            vec![Acq("b"), Tick(TTL + 1)],
            1,
            "NotHeld",
        ),
        (
            "b takes and releases at once",
            vec![Acq("a"), Tick(TTL + 1)],
            vec![Acq("b"), Rel("b")],
            1,
            "NotHeld",
        ),
        (
            "b takes, releases, and a second daemon named a takes",
            vec![Acq("a"), Tick(TTL + 1)],
            vec![Acq("b"), Rel("b"), Acq("a")],
            1,
            "NotHeld",
        ),
        (
            "a second daemon named a renews first (same epoch)",
            vec![Acq("a")],
            vec![RenCur("a")],
            1,
            "Renewed",
        ),
        (
            "a releases itself first (a stop elsewhere under the name)",
            vec![Acq("a")],
            vec![Rel("a")],
            1,
            "Renewed",
        ),
        (
            "the clock only moves (nothing written in between)",
            vec![Acq("a")],
            vec![Tick(5 * TTL)],
            1,
            "Renewed",
        ),
    ];
    for (what, prefix, inj, epoch, want) in cases {
        let mut tally = Tally::default();
        let shared: Arc<dyn ObjectStore> = Arc::new(ContentEtag::default());
        e1_case(shared, &prefix, epoch, &inj, &mut tally, what).await;
        println!(
            "## E3 {what}: renewed {}, not held {}, refused then judged {}, mismatches {}",
            tally.renewed,
            tally.not_held,
            tally.refused_then_judged,
            tally.mismatches.len()
        );
        for m in &tally.mismatches {
            println!("  {m}");
        }
        assert!(tally.mismatches.is_empty(), "{what}");
        assert_eq!(
            (tally.renewed, tally.not_held),
            if want == "Renewed" { (1, 0) } else { (0, 1) },
            "{what}"
        );
    }
}

// ---------------------------------------------------------------------
// E4: the pull request's own conformance case over a live S3 endpoint
// ---------------------------------------------------------------------

#[tokio::test]
async fn e4_the_renewal_conformance_over_live_s3() {
    let Ok(endpoint) = std::env::var("REVIEW_S3_ENDPOINT") else {
        println!("## E4 live S3: skipped, REVIEW_S3_ENDPOINT not set");
        return;
    };
    let bucket = format!("review-f5c-{}", ulid::Ulid::new().to_string().to_lowercase());
    let store = skynbd_store::S3Store::new(&endpoint, &bucket, "test", "test", "us-east-1", true)
        .await;
    store.ensure_bucket().await.expect("the bucket");
    let now = Arc::new(AtomicU64::new(T0));
    let s = ObjectRefStore::new(store).with_clock(clock_of(&now));
    let n = Arc::clone(&now);
    let advance = move |ms: u64| _ = n.fetch_add(ms, Ordering::SeqCst);
    skynbd_refs::conformance::exercise_lease(&s, &advance).await;
    skynbd_refs::conformance::exercise_lease_renew(&s, &advance).await;
    println!("## E4 live S3: exercise_lease and exercise_lease_renew pass");
}
