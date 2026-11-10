//! Repointable origin export conformance (multi-node design D-M15,
//! task 15a).
//!
//! The claims under test, each mapped to a design invariant:
//! - a live NBD connection follows a repoint without reconnecting
//!   (Invariant A: the swap happens under the engine `Arc` every
//!   attached connection shares);
//! - the export is read-only on the wire;
//! - a fresh registry (skynbdd restart, `-persist` reconnect) resolves
//!   `<vol>/origin` through the pointer row, not the newest generation
//!   (Invariant D / §6.3 reconnect surface);
//! - a stale pointer falls back to the newest generation, an absent
//!   pointer serves nothing (the export appears only once the pointer
//!   is initialized).

use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::proto::*;
use skynbd_nbd::{origin, spawn_registry_sync, DynEngine, ExportRegistry, Server, VolumeBinding};
use skynbd_refs::{MemoryRefStore, RefOp, RefStore, TxnOutcome};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::{CommitHash, VolumeId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const VOL_BYTES: u64 = 256 * 1024;
const CHUNK: usize = 8192;

fn pattern(seed: u8) -> Vec<u8> {
    (0..CHUNK).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// A lineage volume with two published generations: g0 carries
/// `pattern(0xA0)` at offset 0, g1 overwrites it with `pattern(0xB1)`.
struct Fixture {
    store: Arc<dyn ObjectStore>,
    refs: Arc<dyn RefStore>,
    vol: VolumeId,
    g0: CommitHash,
    g1: CommitHash,
}

impl Fixture {
    async fn new() -> Self {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
        let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
        let vol = VolumeId(ulid::Ulid::new());
        let mut engine: DynEngine = Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol,
            VOL_BYTES,
            EngineConfig::default(),
        )
        .await
        .unwrap();
        engine.write(0, &pattern(0xA0)).unwrap();
        let (_, g0) = engine.snapshot_create("g0").await.unwrap();
        engine.write(0, &pattern(0xB1)).unwrap();
        let (_, g1) = engine.snapshot_create("g1").await.unwrap();
        Self {
            store,
            refs,
            vol,
            g0,
            g1,
        }
    }

    fn binding(&self) -> VolumeBinding {
        VolumeBinding {
            vol_id: self.vol,
            vol_name: "lin".into(),
            store: Arc::clone(&self.store),
            refs: Arc::clone(&self.refs),
            cfg: EngineConfig::default(),
        }
    }

    async fn set_pointer(&self, ref_name: &str) {
        self.refs
            .transact(
                self.vol,
                vec![RefOp::MetaPut {
                    key: origin::ORIGIN_META_KEY.into(),
                    value: ref_name.as_bytes().to_vec(),
                }],
            )
            .await
            .unwrap();
    }

    async fn served_commit(&self, registry: &ExportRegistry) -> Option<CommitHash> {
        let export = registry.get("lin/origin").await?;
        let commit = export.engine.write().await.head_commit_hash();
        Some(commit)
    }
}

async fn serve(registry: Arc<ExportRegistry>) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(Server::new(registry).run(listener));
    addr
}

// ---------------------------------------------------- minimal NBD client

/// Fixed-newstyle handshake + GO. Returns (stream, size, tflags).
async fn connect(addr: std::net::SocketAddr, export: &str) -> Result<(TcpStream, u64, u16), u32> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    assert_eq!(stream.read_u64().await.unwrap(), NBDMAGIC);
    assert_eq!(stream.read_u64().await.unwrap(), IHAVEOPT);
    let _ = stream.read_u16().await.unwrap();
    stream
        .write_u32(CFLAG_FIXED_NEWSTYLE | CFLAG_NO_ZEROES)
        .await
        .unwrap();

    let mut payload = Vec::new();
    payload.extend_from_slice(&(export.len() as u32).to_be_bytes());
    payload.extend_from_slice(export.as_bytes());
    payload.extend_from_slice(&0u16.to_be_bytes());
    stream.write_u64(IHAVEOPT).await.unwrap();
    stream.write_u32(OPT_GO).await.unwrap();
    stream.write_u32(payload.len() as u32).await.unwrap();
    stream.write_all(&payload).await.unwrap();

    let (mut size, mut tflags) = (0u64, 0u16);
    loop {
        assert_eq!(stream.read_u64().await.unwrap(), OPT_REPLY_MAGIC);
        assert_eq!(stream.read_u32().await.unwrap(), OPT_GO);
        let reply_type = stream.read_u32().await.unwrap();
        let len = stream.read_u32().await.unwrap();
        let mut data = vec![0u8; len as usize];
        stream.read_exact(&mut data).await.unwrap();
        match reply_type {
            REP_INFO => {
                if u16::from_be_bytes(data[0..2].try_into().unwrap()) == INFO_EXPORT {
                    size = u64::from_be_bytes(data[2..10].try_into().unwrap());
                    tflags = u16::from_be_bytes(data[10..12].try_into().unwrap());
                }
            }
            REP_ACK => return Ok((stream, size, tflags)),
            err => return Err(err),
        }
    }
}

async fn cmd(stream: &mut TcpStream, c: u16, offset: u64, len: u32, payload: &[u8]) -> u32 {
    let mut hdr = [0u8; 28];
    hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
    hdr[6..8].copy_from_slice(&c.to_be_bytes());
    hdr[8..16].copy_from_slice(&9u64.to_be_bytes());
    hdr[16..24].copy_from_slice(&offset.to_be_bytes());
    hdr[24..28].copy_from_slice(&len.to_be_bytes());
    stream.write_all(&hdr).await.unwrap();
    if !payload.is_empty() {
        stream.write_all(payload).await.unwrap();
    }
    assert_eq!(stream.read_u32().await.unwrap(), SIMPLE_REPLY_MAGIC);
    let err = stream.read_u32().await.unwrap();
    assert_eq!(stream.read_u64().await.unwrap(), 9);
    err
}

async fn read(stream: &mut TcpStream, offset: u64, len: u32) -> Vec<u8> {
    assert_eq!(cmd(stream, CMD_READ, offset, len, &[]).await, 0);
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await.unwrap();
    buf
}

// ---------------------------------------------------------------- tests

/// Invariant A on the wire: one connection, two generations. The client
/// attaches while the pointer is at g0, the repoint lands, and the very
/// same connection reads g1 content — no reconnect, no re-negotiation.
/// Also: the export is read-only (tflags + EPERM on write).
#[tokio::test(flavor = "multi_thread")]
async fn repoint_swaps_under_a_live_connection() {
    let f = Fixture::new().await;
    f.set_pointer("snap/g0").await;
    let registry = Arc::new(ExportRegistry::new());
    origin::reconcile(&registry, &f.binding()).await.unwrap();
    let addr = serve(Arc::clone(&registry)).await;

    let (mut c, size, tflags) = connect(addr, "lin/origin").await.unwrap();
    assert_eq!(size, VOL_BYTES);
    assert_ne!(tflags & TFLAG_READ_ONLY, 0, "origin must be read-only");
    assert_eq!(read(&mut c, 0, CHUNK as u32).await, pattern(0xA0));
    assert_eq!(
        cmd(&mut c, CMD_WRITE, 0, 512, &[0u8; 512]).await,
        EPERM,
        "write on the origin export must be refused"
    );

    // The advance's step 4, as the API performs it: row + live swap.
    f.set_pointer("snap/g1").await;
    origin::repoint(&registry, &f.binding(), f.g1)
        .await
        .unwrap();

    assert_eq!(
        read(&mut c, 0, CHUNK as u32).await,
        pattern(0xB1),
        "attached connection must serve the new generation after repoint"
    );
    // Idempotent replay of the same repoint is a no-op.
    origin::repoint(&registry, &f.binding(), f.g1)
        .await
        .unwrap();
    assert_eq!(f.served_commit(&registry).await, Some(f.g1));
}

/// Invariant D across restart: a fresh registry + sync task (what a
/// skynbdd restart builds before a `-persist` client reconnects) serves
/// the generation the pointer row names — g0, even though g1 is newer.
#[tokio::test(flavor = "multi_thread")]
async fn restart_resolves_through_the_pointer_row() {
    let f = Fixture::new().await;
    f.set_pointer("snap/g0").await;

    let registry = Arc::new(ExportRegistry::new());
    spawn_registry_sync(Arc::clone(&registry), f.binding());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.served_commit(&registry).await.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "origin export never appeared after restart"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        f.served_commit(&registry).await,
        Some(f.g0),
        "restart must serve the pointer's generation, not the newest"
    );

    let addr = serve(Arc::clone(&registry)).await;
    let (mut c, _, _) = connect(addr, "lin/origin").await.unwrap();
    assert_eq!(read(&mut c, 0, CHUNK as u32).await, pattern(0xA0));
}

/// The absent/stale rules: no pointer → no export (an uninitialized
/// origin must not silently track "newest"); a stale pointer resolves to
/// the newest generation (anything older would resurrect old data); a
/// deleted pointer takes the export down again.
#[tokio::test(flavor = "multi_thread")]
async fn pointer_absent_and_stale_rules() {
    let f = Fixture::new().await;
    let registry = Arc::new(ExportRegistry::new());
    let b = f.binding();

    origin::reconcile(&registry, &b).await.unwrap();
    assert_eq!(f.served_commit(&registry).await, None);
    let addr = serve(Arc::clone(&registry)).await;
    assert_eq!(
        connect(addr, "lin/origin").await.unwrap_err(),
        REP_ERR_UNKNOWN,
        "uninitialized origin must not be attachable"
    );

    f.set_pointer("snap/gone").await;
    origin::reconcile(&registry, &b).await.unwrap();
    assert_eq!(
        f.served_commit(&registry).await,
        Some(f.g1),
        "stale pointer must resolve to the newest generation"
    );

    f.refs
        .transact(
            f.vol,
            vec![RefOp::MetaDelete {
                key: origin::ORIGIN_META_KEY.into(),
            }],
        )
        .await
        .unwrap();
    origin::reconcile(&registry, &b).await.unwrap();
    assert_eq!(f.served_commit(&registry).await, None);
}

/// Invariant E stale-view matrix (15g): the §3 windows that can strand a
/// resolution surface — pre-repoint (gₜ published, row still gₒ) and
/// post-repoint — crossed with the three surfaces: same-server restart,
/// failover to a fresh replica, and a `-persist` reconnect. Every surface
/// serves exactly what the pointer row names — never a warm view, never
/// "newest" — and the row itself only moves forward: the `MetaCas` guard
/// closes the fence's TOCTOU ("once served ≥ gₜ has been observed it can
/// never become false").
#[tokio::test(flavor = "multi_thread")]
async fn stale_view_matrix() {
    let f = Fixture::new().await;

    // ---- Window 1: g1 is published, the pointer still names g0 (the
    // crash between publish and repoint).
    f.set_pointer("snap/g0").await;

    let restart = Arc::new(ExportRegistry::new());
    origin::reconcile(&restart, &f.binding()).await.unwrap();
    assert_eq!(
        f.served_commit(&restart).await,
        Some(f.g0),
        "restart in window 1 must serve the row, not the newest generation"
    );
    let replica = Arc::new(ExportRegistry::new());
    origin::reconcile(&replica, &f.binding()).await.unwrap();
    assert_eq!(
        f.served_commit(&replica).await,
        Some(f.g0),
        "failover replica, window 1"
    );
    let addr = serve(Arc::clone(&restart)).await;
    let (mut c1, _, _) = connect(addr, "lin/origin").await.unwrap();
    assert_eq!(
        read(&mut c1, 0, CHUNK as u32).await,
        pattern(0xA0),
        "reconnect, window 1"
    );

    // ---- The advance's repoint, exactly as the API writes it (15g):
    // pin-new + MetaCas(row: g0 → g1) + unpin-old — then the live swap on
    // the serving registry only. The replica deliberately misses it.
    let out = f
        .refs
        .transact(
            f.vol,
            vec![
                RefOp::Pin {
                    name: "snap/g1".into(),
                    holder: origin::ORIGIN_PIN_HOLDER.into(),
                },
                RefOp::MetaCas {
                    key: origin::ORIGIN_META_KEY.into(),
                    expected: Some(b"snap/g0".to_vec()),
                    value: b"snap/g1".to_vec(),
                },
                RefOp::Unpin {
                    name: "snap/g0".into(),
                    holder: origin::ORIGIN_PIN_HOLDER.into(),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(out, TxnOutcome::Committed);
    origin::repoint(&restart, &f.binding(), f.g1).await.unwrap();

    // ---- Window 2, same surfaces. The attached connection follows the
    // swap; a fresh reconnect resolves the new view; the replica that
    // never saw the repoint call converges on its next reconcile
    // (startup sync / watch-lag resync) — through the row, never a warm
    // view; a brand-new restart lands directly on g1.
    assert_eq!(
        read(&mut c1, 0, CHUNK as u32).await,
        pattern(0xB1),
        "attached, window 2"
    );
    let (mut c2, _, _) = connect(addr, "lin/origin").await.unwrap();
    assert_eq!(
        read(&mut c2, 0, CHUNK as u32).await,
        pattern(0xB1),
        "reconnect, window 2"
    );
    origin::reconcile(&replica, &f.binding()).await.unwrap();
    assert_eq!(
        f.served_commit(&replica).await,
        Some(f.g1),
        "replica converges"
    );
    let restart2 = Arc::new(ExportRegistry::new());
    origin::reconcile(&restart2, &f.binding()).await.unwrap();
    assert_eq!(
        f.served_commit(&restart2).await,
        Some(f.g1),
        "restart, window 2"
    );

    // ---- The write side: a replayed pre-advance repoint (its MetaCas
    // still expects g0) conflicts, applies nothing, and every surface
    // keeps serving g1.
    let out = f
        .refs
        .transact(
            f.vol,
            vec![
                RefOp::Pin {
                    name: "snap/g0".into(),
                    holder: origin::ORIGIN_PIN_HOLDER.into(),
                },
                RefOp::MetaCas {
                    key: origin::ORIGIN_META_KEY.into(),
                    expected: Some(b"snap/g0".to_vec()),
                    value: b"snap/g0".to_vec(),
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(out, TxnOutcome::Conflict { op_index: 1, .. }));
    assert_eq!(
        f.refs
            .meta_get(f.vol, origin::ORIGIN_META_KEY)
            .await
            .unwrap()
            .unwrap(),
        b"snap/g1"
    );
    assert!(
        f.refs.pins(f.vol, "snap/g0").await.unwrap().is_empty(),
        "aborted replay must leave no stray pin"
    );
    origin::reconcile(&restart2, &f.binding()).await.unwrap();
    assert_eq!(f.served_commit(&restart2).await, Some(f.g1));
}

/// The resolution helper itself: pointer wins while live, `initialized`
/// distinguishes the fallback, and no-generations resolves to nothing.
#[tokio::test]
async fn resolve_reports_initialization() {
    let f = Fixture::new().await;

    let r = origin::resolve(&f.refs, f.vol).await.unwrap().unwrap();
    assert_eq!((r.ref_name.as_str(), r.initialized), ("snap/g1", false));

    f.set_pointer("snap/g0").await;
    let r = origin::resolve(&f.refs, f.vol).await.unwrap().unwrap();
    assert_eq!((r.ref_name.as_str(), r.initialized), ("snap/g0", true));
    assert_eq!(r.commit, f.g0);

    // A volume with no generations at all resolves to nothing.
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let empty = VolumeId(ulid::Ulid::new());
    assert!(origin::resolve(&refs, empty).await.unwrap().is_none());
}
