//! NBD server interop tests (design §14.4 scenarios S1–S2 at Tier A).
//!
//! Two layers:
//! 1. A minimal in-crate NBD client (raw sockets) exercising negotiation,
//!    IO semantics, RO enforcement, and crash/restart durability — always
//!    runs.
//! 2. Real-client interop via `qemu-io` / `nbdinfo` — runs when the
//!    binaries are present (CI installs them; skipped otherwise).

use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::proto::*;
use skynbd_nbd::{DynEngine, Export, ExportRegistry, Server};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const VOL_BYTES: u64 = 256 * 1024;

struct TestServer {
    addr: std::net::SocketAddr,
    registry: Arc<ExportRegistry>,
    store: Arc<dyn ObjectStore>,
    refs: Arc<dyn RefStore>,
    vol: VolumeId,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
        let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
        let vol = VolumeId(ulid::Ulid::new());
        let engine: DynEngine = Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol,
            VOL_BYTES,
            EngineConfig::default(),
        )
        .await
        .unwrap();

        let registry = Arc::new(ExportRegistry::new());
        registry
            .register(
                "vol",
                Export {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                    read_only: false,
                    size: VOL_BYTES,
                },
            )
            .await;

        Self::serve(registry, store, refs, vol).await
    }

    /// Every scenario runs with concurrent request servicing on (issue
    /// #97 item 3): a client that waits for each reply sees exactly the
    /// serial behavior, so the whole suite also covers the new path.
    async fn serve(
        registry: Arc<ExportRegistry>,
        store: Arc<dyn ObjectStore>,
        refs: Arc<dyn RefStore>,
        vol: VolumeId,
    ) -> Self {
        Self::serve_with(registry, store, refs, vol, 16).await
    }

    async fn serve_with(
        registry: Arc<ExportRegistry>,
        store: Arc<dyn ObjectStore>,
        refs: Arc<dyn RefStore>,
        vol: VolumeId,
        max_in_flight: usize,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(
            Server::new(Arc::clone(&registry))
                .max_in_flight(max_in_flight)
                .run(listener),
        );
        Self {
            addr,
            registry,
            store,
            refs,
            vol,
            task,
        }
    }

    /// Simulate `kill -9`: abort the accept loop and drop every engine
    /// (registry cleared), keeping only durable state (store + refs).
    async fn kill_dash_nine(self) -> (Arc<dyn ObjectStore>, Arc<dyn RefStore>, VolumeId) {
        self.task.abort();
        (self.store, self.refs, self.vol)
    }

    /// "Restart the daemon": reopen the volume purely from durable state.
    async fn restart(store: Arc<dyn ObjectStore>, refs: Arc<dyn RefStore>, vol: VolumeId) -> Self {
        let engine: DynEngine = Engine::open(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol,
            EngineConfig::default(),
        )
        .await
        .unwrap();
        let registry = Arc::new(ExportRegistry::new());
        registry
            .register(
                "vol",
                Export {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                    read_only: false,
                    size: VOL_BYTES,
                },
            )
            .await;
        Self::serve(registry, store, refs, vol).await
    }
}

// ------------------------------------------------------- minimal client

struct Client {
    stream: TcpStream,
    next_cookie: u64,
}

impl Client {
    /// Full fixed-newstyle handshake + NBD_OPT_GO. Returns (size, tflags).
    async fn connect(addr: std::net::SocketAddr, export: &str) -> Result<(Self, u64, u16), u32> {
        let mut stream = TcpStream::connect(addr).await.unwrap();

        assert_eq!(stream.read_u64().await.unwrap(), NBDMAGIC);
        assert_eq!(stream.read_u64().await.unwrap(), IHAVEOPT);
        let hflags = stream.read_u16().await.unwrap();
        assert_ne!(hflags & FLAG_FIXED_NEWSTYLE, 0);
        stream
            .write_u32(CFLAG_FIXED_NEWSTYLE | CFLAG_NO_ZEROES)
            .await
            .unwrap();

        // NBD_OPT_GO with zero info requests.
        let mut payload = Vec::new();
        payload.extend_from_slice(&(export.len() as u32).to_be_bytes());
        payload.extend_from_slice(export.as_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());
        write_option(&mut stream, OPT_GO, &payload).await;

        let mut size = 0u64;
        let mut tflags = 0u16;
        loop {
            let (reply_type, data) = read_opt_reply(&mut stream, OPT_GO).await;
            match reply_type {
                REP_INFO => {
                    let info = u16::from_be_bytes(data[0..2].try_into().unwrap());
                    if info == INFO_EXPORT {
                        size = u64::from_be_bytes(data[2..10].try_into().unwrap());
                        tflags = u16::from_be_bytes(data[10..12].try_into().unwrap());
                    }
                }
                REP_ACK => {
                    return Ok((
                        Self {
                            stream,
                            next_cookie: 1,
                        },
                        size,
                        tflags,
                    ))
                }
                err => return Err(err),
            }
        }
    }

    async fn cmd(&mut self, cmd: u16, flags: u16, offset: u64, len: u32, payload: &[u8]) -> u32 {
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        let mut hdr = [0u8; 28];
        hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
        hdr[4..6].copy_from_slice(&flags.to_be_bytes());
        hdr[6..8].copy_from_slice(&cmd.to_be_bytes());
        hdr[8..16].copy_from_slice(&cookie.to_be_bytes());
        hdr[16..24].copy_from_slice(&offset.to_be_bytes());
        hdr[24..28].copy_from_slice(&len.to_be_bytes());
        self.stream.write_all(&hdr).await.unwrap();
        if !payload.is_empty() {
            self.stream.write_all(payload).await.unwrap();
        }
        assert_eq!(self.stream.read_u32().await.unwrap(), SIMPLE_REPLY_MAGIC);
        let err = self.stream.read_u32().await.unwrap();
        assert_eq!(
            self.stream.read_u64().await.unwrap(),
            cookie,
            "cookie mismatch"
        );
        err
    }

    async fn write(&mut self, offset: u64, data: &[u8]) -> u32 {
        self.cmd(CMD_WRITE, 0, offset, data.len() as u32, data)
            .await
    }

    async fn read(&mut self, offset: u64, len: u32) -> Result<Vec<u8>, u32> {
        let err = self.cmd(CMD_READ, 0, offset, len, &[]).await;
        if err != 0 {
            return Err(err);
        }
        let mut buf = vec![0u8; len as usize];
        self.stream.read_exact(&mut buf).await.unwrap();
        Ok(buf)
    }

    async fn flush(&mut self) -> u32 {
        self.cmd(CMD_FLUSH, 0, 0, 0, &[]).await
    }

    async fn disc(mut self) {
        let mut hdr = [0u8; 28];
        hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
        hdr[6..8].copy_from_slice(&CMD_DISC.to_be_bytes());
        self.stream.write_all(&hdr).await.unwrap();
    }
}

async fn write_option(stream: &mut TcpStream, option: u32, payload: &[u8]) {
    stream.write_u64(IHAVEOPT).await.unwrap();
    stream.write_u32(option).await.unwrap();
    stream.write_u32(payload.len() as u32).await.unwrap();
    stream.write_all(payload).await.unwrap();
}

async fn read_opt_reply(stream: &mut TcpStream, expect_option: u32) -> (u32, Vec<u8>) {
    assert_eq!(stream.read_u64().await.unwrap(), OPT_REPLY_MAGIC);
    assert_eq!(stream.read_u32().await.unwrap(), expect_option);
    let reply_type = stream.read_u32().await.unwrap();
    let len = stream.read_u32().await.unwrap();
    let mut data = vec![0u8; len as usize];
    stream.read_exact(&mut data).await.unwrap();
    (reply_type, data)
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

// ------------------------------------------------------- protocol tests

/// S1 core: negotiate, write, flush, read back, reconnect, read again.
#[tokio::test(flavor = "multi_thread")]
async fn s1_basic_io_roundtrip() {
    let srv = TestServer::start().await;
    let (mut c, size, tflags) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(size, VOL_BYTES);
    assert_ne!(tflags & TFLAG_SEND_FLUSH, 0);
    assert_ne!(tflags & TFLAG_SEND_FUA, 0);
    assert_eq!(tflags & TFLAG_READ_ONLY, 0);

    let data = pattern(8192, 0x5A);
    assert_eq!(c.write(4096, &data).await, 0);
    assert_eq!(c.read(4096, 8192).await.unwrap(), data); // I3 over the wire
    assert_eq!(c.flush().await, 0);
    c.disc().await;

    // Reconnect: flushed data still there (served from committed state).
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.read(4096, 8192).await.unwrap(), data);
    assert_eq!(c.read(0, 4096).await.unwrap(), vec![0u8; 4096]);
}

/// S2 core: flushed data survives kill -9 + restart; unflushed writes and
/// courtesy-commit-free teardown behave per D1.
#[tokio::test(flavor = "multi_thread")]
async fn s2_flush_durability_across_kill() {
    let srv = TestServer::start().await;
    let p1 = pattern(4096, 1);
    let p2 = pattern(4096, 2);

    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.write(0, &p1).await, 0);
    assert_eq!(c.flush().await, 0);
    assert_eq!(c.write(65536, &p2).await, 0); // never flushed
                                              // No DISC: connection dies with the server (kill -9 semantics).

    let (store, refs, vol) = srv.kill_dash_nine().await;
    let srv = TestServer::restart(store, refs, vol).await;

    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(
        c.read(0, 4096).await.unwrap(),
        p1,
        "flushed write lost (I4)"
    );
    assert_eq!(
        c.read(65536, 4096).await.unwrap(),
        vec![0u8; 4096],
        "unflushed write must not survive (I7/D1)"
    );
}

/// FUA writes are durable without an explicit flush (design §7.1).
#[tokio::test(flavor = "multi_thread")]
async fn fua_write_survives_kill() {
    let srv = TestServer::start().await;
    let data = pattern(2048, 9);
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.cmd(CMD_WRITE, CMD_FLAG_FUA, 8192, 2048, &data).await, 0);

    let (store, refs, vol) = srv.kill_dash_nine().await;
    let srv = TestServer::restart(store, refs, vol).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.read(8192, 2048).await.unwrap(), data);
}

/// TRIM and WRITE_ZEROES read back as zeros (design §7.1).
#[tokio::test(flavor = "multi_thread")]
async fn trim_and_zeroes_read_zero() {
    let srv = TestServer::start().await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let data = pattern(8192, 7);
    assert_eq!(c.write(0, &data).await, 0);
    assert_eq!(c.cmd(CMD_TRIM, 0, 0, 4096, &[]).await, 0);
    assert_eq!(c.cmd(CMD_WRITE_ZEROES, 0, 4096, 2048, &[]).await, 0);
    let mut expect = data.clone();
    expect[0..6144].fill(0);
    assert_eq!(c.read(0, 8192).await.unwrap(), expect);
}

/// Error mapping (design §8.3): bounds, alignment, zero length, RO.
#[tokio::test(flavor = "multi_thread")]
async fn error_mapping() {
    let srv = TestServer::start().await;

    // Register an RO alias of the same engine.
    let rw = srv.registry.get("vol").await.unwrap();
    srv.registry
        .register(
            "vol-ro",
            Export {
                read_only: true,
                ..rw
            },
        )
        .await;

    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.write(VOL_BYTES - 512, &pattern(1024, 0)).await, EINVAL); // oob
    assert_eq!(c.write(100, &pattern(512, 0)).await, EINVAL); // unaligned
    assert_eq!(c.cmd(CMD_READ, 0, 0, 0, &[]).await, EINVAL); // zero-len
    assert_eq!(c.cmd(CMD_FLUSH, 0, 512, 0, &[]).await, EINVAL); // nonzero offset
    assert_eq!(c.cmd(CMD_CACHE, 0, 0, 512, &[]).await, ENOTSUP);

    let (mut ro, _, tflags) = Client::connect(srv.addr, "vol-ro").await.unwrap();
    assert_ne!(tflags & TFLAG_READ_ONLY, 0);
    assert_eq!(ro.write(0, &pattern(512, 0)).await, EPERM);
    assert_eq!(ro.cmd(CMD_TRIM, 0, 0, 512, &[]).await, EPERM);
    assert_eq!(ro.flush().await, 0); // flush on RO is a successful no-op
    assert!(ro.read(0, 512).await.is_ok());

    // Unaligned READ is tolerated via the shim.
    assert_eq!(c.read(100, 512).await.unwrap().len(), 512);
}

/// Negotiation surface: LIST, INFO-without-GO, unknown export, ABORT.
#[tokio::test(flavor = "multi_thread")]
async fn negotiation_list_info_unknown_abort() {
    let srv = TestServer::start().await;
    let mut stream = TcpStream::connect(srv.addr).await.unwrap();
    assert_eq!(stream.read_u64().await.unwrap(), NBDMAGIC);
    assert_eq!(stream.read_u64().await.unwrap(), IHAVEOPT);
    let _ = stream.read_u16().await.unwrap();
    stream.write_u32(CFLAG_FIXED_NEWSTYLE).await.unwrap();

    // LIST
    write_option(&mut stream, OPT_LIST, &[]).await;
    let (t, data) = read_opt_reply(&mut stream, OPT_LIST).await;
    assert_eq!(t, REP_SERVER);
    assert_eq!(&data[4..], b"vol");
    let (t, _) = read_opt_reply(&mut stream, OPT_LIST).await;
    assert_eq!(t, REP_ACK);

    // GO for an unknown export: error, connection survives.
    let mut payload = Vec::new();
    payload.extend_from_slice(&4u32.to_be_bytes());
    payload.extend_from_slice(b"nope");
    payload.extend_from_slice(&0u16.to_be_bytes());
    write_option(&mut stream, OPT_GO, &payload).await;
    let (t, _) = read_opt_reply(&mut stream, OPT_GO).await;
    assert_eq!(t, REP_ERR_UNKNOWN);

    // STRUCTURED_REPLY: supported as of M4 (design §8.3).
    write_option(&mut stream, OPT_STRUCTURED_REPLY, &[]).await;
    let (t, _) = read_opt_reply(&mut stream, OPT_STRUCTURED_REPLY).await;
    assert_eq!(t, REP_ACK);
    // SET_META_CONTEXT for an unknown context list still ACKs with no
    // context; a truly unknown option stays ERR_UNSUP.
    write_option(&mut stream, 999, &[]).await;
    let (t, _) = read_opt_reply(&mut stream, 999).await;
    assert_eq!(t, REP_ERR_UNSUP);

    // ABORT acks then closes.
    write_option(&mut stream, OPT_ABORT, &[]).await;
    let (t, _) = read_opt_reply(&mut stream, OPT_ABORT).await;
    assert_eq!(t, REP_ACK);
}

// ------------------------------------------------ structured replies (M4)

/// Handshake negotiating structured replies + base:allocation, then GO.
async fn connect_structured(addr: std::net::SocketAddr, export: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    assert_eq!(stream.read_u64().await.unwrap(), NBDMAGIC);
    assert_eq!(stream.read_u64().await.unwrap(), IHAVEOPT);
    let _ = stream.read_u16().await.unwrap();
    stream
        .write_u32(CFLAG_FIXED_NEWSTYLE | CFLAG_NO_ZEROES)
        .await
        .unwrap();

    write_option(&mut stream, OPT_STRUCTURED_REPLY, &[]).await;
    let (t, _) = read_opt_reply(&mut stream, OPT_STRUCTURED_REPLY).await;
    assert_eq!(t, REP_ACK);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(export.len() as u32).to_be_bytes());
    payload.extend_from_slice(export.as_bytes());
    payload.extend_from_slice(&1u32.to_be_bytes());
    payload.extend_from_slice(&(BASE_ALLOCATION.len() as u32).to_be_bytes());
    payload.extend_from_slice(BASE_ALLOCATION.as_bytes());
    write_option(&mut stream, OPT_SET_META_CONTEXT, &payload).await;
    let (t, data) = read_opt_reply(&mut stream, OPT_SET_META_CONTEXT).await;
    assert_eq!(t, REP_META_CONTEXT);
    assert_eq!(
        u32::from_be_bytes(data[0..4].try_into().unwrap()),
        ALLOCATION_CTX_ID
    );
    assert_eq!(&data[4..], BASE_ALLOCATION.as_bytes());
    let (t, _) = read_opt_reply(&mut stream, OPT_SET_META_CONTEXT).await;
    assert_eq!(t, REP_ACK);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(export.len() as u32).to_be_bytes());
    payload.extend_from_slice(export.as_bytes());
    payload.extend_from_slice(&0u16.to_be_bytes());
    write_option(&mut stream, OPT_GO, &payload).await;
    loop {
        let (t, _) = read_opt_reply(&mut stream, OPT_GO).await;
        if t == REP_ACK {
            return stream;
        }
        assert_eq!(t, REP_INFO);
    }
}

async fn send_request(stream: &mut TcpStream, cmd: u16, flags: u16, offset: u64, len: u32) {
    let mut hdr = [0u8; 28];
    hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
    hdr[4..6].copy_from_slice(&flags.to_be_bytes());
    hdr[6..8].copy_from_slice(&cmd.to_be_bytes());
    hdr[8..16].copy_from_slice(&7u64.to_be_bytes());
    hdr[16..24].copy_from_slice(&offset.to_be_bytes());
    hdr[24..28].copy_from_slice(&len.to_be_bytes());
    stream.write_all(&hdr).await.unwrap();
}

async fn send_request_c(
    stream: &mut TcpStream,
    cmd: u16,
    flags: u16,
    offset: u64,
    len: u32,
    cookie: u64,
) {
    let mut hdr = [0u8; 28];
    hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
    hdr[4..6].copy_from_slice(&flags.to_be_bytes());
    hdr[6..8].copy_from_slice(&cmd.to_be_bytes());
    hdr[8..16].copy_from_slice(&cookie.to_be_bytes());
    hdr[16..24].copy_from_slice(&offset.to_be_bytes());
    hdr[24..28].copy_from_slice(&len.to_be_bytes());
    stream.write_all(&hdr).await.unwrap();
}

/// Read one structured reply, any cookie: (cookie, flags, type, payload).
async fn read_structured_c(stream: &mut TcpStream) -> (u64, u16, u16, Vec<u8>) {
    assert_eq!(stream.read_u32().await.unwrap(), STRUCTURED_REPLY_MAGIC);
    let flags = stream.read_u16().await.unwrap();
    let rtype = stream.read_u16().await.unwrap();
    let cookie = stream.read_u64().await.unwrap();
    let len = stream.read_u32().await.unwrap();
    let mut data = vec![0u8; len as usize];
    stream.read_exact(&mut data).await.unwrap();
    (cookie, flags, rtype, data)
}

/// Read one structured reply: (flags, type, payload).
async fn read_structured(stream: &mut TcpStream) -> (u16, u16, Vec<u8>) {
    assert_eq!(stream.read_u32().await.unwrap(), STRUCTURED_REPLY_MAGIC);
    let flags = stream.read_u16().await.unwrap();
    let rtype = stream.read_u16().await.unwrap();
    assert_eq!(stream.read_u64().await.unwrap(), 7);
    let len = stream.read_u32().await.unwrap();
    let mut data = vec![0u8; len as usize];
    stream.read_exact(&mut data).await.unwrap();
    (flags, rtype, data)
}

/// BLOCK_STATUS end to end: allocation runs reflect data/zero/hole
/// (design §8.3), structured READ works, REQ_ONE truncates.
#[tokio::test(flavor = "multi_thread")]
async fn structured_block_status_and_read() {
    let srv = TestServer::start().await;

    // Layout: data [0,64k), explicit zeroes [64k,128k), hole [128k,256k).
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.write(0, &pattern(65536, 3)).await, 0);
    assert_eq!(c.cmd(CMD_WRITE_ZEROES, 0, 65536, 65536, &[]).await, 0);
    assert_eq!(c.flush().await, 0);
    c.disc().await;

    let mut st = connect_structured(srv.addr, "vol").await;

    // Whole-device BLOCK_STATUS: [(64k, allocated), (192k, HOLE|ZERO)].
    send_request(&mut st, CMD_BLOCK_STATUS, 0, 0, VOL_BYTES as u32).await;
    let (flags, rtype, data) = read_structured(&mut st).await;
    assert_eq!(flags & REPLY_FLAG_DONE, REPLY_FLAG_DONE);
    assert_eq!(rtype, REPLY_TYPE_BLOCK_STATUS);
    assert_eq!(
        u32::from_be_bytes(data[0..4].try_into().unwrap()),
        ALLOCATION_CTX_ID
    );
    let descs: Vec<(u32, u32)> = data[4..]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            (
                u32::from_be_bytes(c[0..4].try_into().unwrap()),
                u32::from_be_bytes(c[4..8].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(
        descs,
        vec![
            (65536, 0),
            (VOL_BYTES as u32 - 65536, STATE_HOLE | STATE_ZERO)
        ]
    );

    // REQ_ONE: exactly one descriptor.
    send_request(
        &mut st,
        CMD_BLOCK_STATUS,
        CMD_FLAG_REQ_ONE,
        0,
        VOL_BYTES as u32,
    )
    .await;
    let (_, rtype, data) = read_structured(&mut st).await;
    assert_eq!(rtype, REPLY_TYPE_BLOCK_STATUS);
    assert_eq!(data.len(), 4 + 8);

    // Structured READ: OFFSET_DATA chunk with the right offset + bytes.
    send_request(&mut st, CMD_READ, 0, 512, 1024).await;
    let (flags, rtype, data) = read_structured(&mut st).await;
    assert_eq!(flags & REPLY_FLAG_DONE, REPLY_FLAG_DONE);
    assert_eq!(rtype, REPLY_TYPE_OFFSET_DATA);
    assert_eq!(u64::from_be_bytes(data[0..8].try_into().unwrap()), 512);
    assert_eq!(&data[8..], &pattern(65536, 3)[512..1536]);

    // BLOCK_STATUS without negotiation → structured error over a plain
    // structured connection is covered; over a simple connection it's a
    // simple EINVAL.
    let (mut plain, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(plain.cmd(CMD_BLOCK_STATUS, 0, 0, 4096, &[]).await, EINVAL);
}

/// qemu-img map exercises the full structured-reply + meta-context path.
#[tokio::test(flavor = "multi_thread")]
async fn qemu_img_map_allocation() {
    if !have("qemu-img") {
        eprintln!("skipped: qemu-img not installed");
        return;
    }
    let srv = TestServer::start().await;
    let uri = format!("nbd://127.0.0.1:{}/vol", srv.addr.port());

    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.write(0, &pattern(65536, 9)).await, 0);
    assert_eq!(c.flush().await, 0);
    c.disc().await;

    let out = tokio::process::Command::new("qemu-img")
        .args(["map", "--output=json", &uri])
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map: serde_json::Value = serde_json::from_slice(&out.stdout).expect("qemu-img map json");
    let entries = map.as_array().unwrap();
    // First 64k mapped as data, remainder reported zero/unallocated.
    assert_eq!(entries[0]["start"], 0);
    assert_eq!(entries[0]["length"], 65536);
    assert_eq!(entries[0]["data"], true);
    assert_eq!(entries[1]["data"], false);
    assert_eq!(entries[1]["zero"], true);
}

// ---------------------------------------------------- real-client interop

fn have(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// S1 with the real qemu-io client (Tier A harness of design §14.3).
#[tokio::test(flavor = "multi_thread")]
async fn qemu_io_s1_interop() {
    if !have("qemu-io") {
        eprintln!("skipped: qemu-io not installed");
        return;
    }
    let srv = TestServer::start().await;
    let uri = format!("nbd://127.0.0.1:{}/vol", srv.addr.port());

    let out = tokio::process::Command::new("qemu-io")
        .args([
            "-f",
            "raw",
            "--cache",
            "none",
            "-c",
            "write -P 0xaa 0 64k",
            "-c",
            "flush",
            "-c",
            "read -P 0xaa 0 64k",
            "-c",
            "write -z 16k 8k",
            "-c",
            "read -P 0x00 16k 8k",
            "-c",
            "read -P 0xaa 24k 8k",
            &uri,
        ])
        .output()
        .await
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "qemu-io failed:\n{combined}");
    assert!(
        !combined.contains("Pattern verification failed"),
        "{combined}"
    );

    // Reconnect with a fresh qemu-io: committed state serves the pattern.
    let out = tokio::process::Command::new("qemu-io")
        .args(["-f", "raw", "-r", "-c", "read -P 0xaa 0 16k", &uri])
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
}

/// Negotiation conformance via nbdinfo (libnbd).
#[tokio::test(flavor = "multi_thread")]
async fn nbdinfo_negotiation_conformance() {
    if !have("nbdinfo") {
        eprintln!("skipped: nbdinfo not installed");
        return;
    }
    let srv = TestServer::start().await;
    let uri = format!("nbd://127.0.0.1:{}/vol", srv.addr.port());

    let out = tokio::process::Command::new("nbdinfo")
        .arg(&uri)
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "nbdinfo failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains(&VOL_BYTES.to_string()),
        "size missing:\n{text}"
    );

    // --list enumerates the registry.
    let out = tokio::process::Command::new("nbdinfo")
        .args(["--list", &format!("nbd://127.0.0.1:{}", srv.addr.port())])
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("vol"));
}

// ------------------------------------------------------- UDS (M10a)

/// S1-shape interop over a Unix domain socket: same protocol bytes, no
/// TCP. qemu speaks `nbd+unix:///<export>?socket=<path>` (design §15
/// M10a gate).
#[tokio::test(flavor = "multi_thread")]
async fn qemu_io_unix_socket_interop() {
    if !have("qemu-io") {
        eprintln!("skipped: qemu-io not installed");
        return;
    }
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::new());
    let engine: DynEngine = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol,
        VOL_BYTES,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    let registry = Arc::new(ExportRegistry::new());
    registry
        .register(
            "vol",
            Export {
                engine: Arc::new(tokio::sync::RwLock::new(engine)),
                read_only: false,
                size: VOL_BYTES,
            },
        )
        .await;

    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("nbd.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let task = tokio::spawn(Server::new(Arc::clone(&registry)).run_unix(listener));

    let uri = format!("nbd+unix:///vol?socket={}", sock.display());
    let out = tokio::process::Command::new("qemu-io")
        .args([
            "-f",
            "raw",
            "--cache",
            "none",
            "-c",
            "write -P 0xcd 0 64k",
            "-c",
            "flush",
            "-c",
            "read -P 0xcd 0 64k",
            "-c",
            "write -z 16k 8k",
            "-c",
            "read -P 0x00 16k 8k",
            &uri,
        ])
        .output()
        .await
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "qemu-io over UDS failed:\n{combined}");
    assert!(
        !combined.contains("Pattern verification failed"),
        "{combined}"
    );

    // nbdinfo over the same socket, if available.
    if have("nbdinfo") {
        let out = tokio::process::Command::new("nbdinfo")
            .arg(format!("nbd+unix:///vol?socket={}", sock.display()))
            .output()
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "nbdinfo over UDS failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    task.abort();
}

/// An object store that holds every `get` for a fixed delay while armed:
/// a cold read is then a known-length wait on the store.
struct SlowGets {
    inner: Arc<dyn ObjectStore>,
    delay: std::time::Duration,
    gate: Arc<SlowGate>,
}

/// `armed`: delay every GET. `started`: notified as each delayed GET
/// begins, so a test can order a request after a cold fetch is in flight
/// instead of sleeping and hoping.
#[derive(Default)]
struct SlowGate {
    armed: std::sync::atomic::AtomicBool,
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl ObjectStore for SlowGets {
    async fn put(&self, key: &str, data: bytes::Bytes) -> skynbd_store::Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(
        &self,
        key: &str,
        range: Option<std::ops::Range<u64>>,
    ) -> skynbd_store::Result<bytes::Bytes> {
        if self.gate.armed.load(std::sync::atomic::Ordering::SeqCst) {
            self.gate.started.notify_waiters();
            tokio::time::sleep(self.delay).await;
        }
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.inner.delete(key).await
    }
}

/// Once `short` is set, every segment GET returns one byte too few: what
/// a truncated object (or a misbehaving backend) looks like to the engine.
struct ShortBodies {
    inner: Arc<dyn ObjectStore>,
    short: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl ObjectStore for ShortBodies {
    async fn put(&self, key: &str, data: bytes::Bytes) -> skynbd_store::Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(
        &self,
        key: &str,
        range: Option<std::ops::Range<u64>>,
    ) -> skynbd_store::Result<bytes::Bytes> {
        let b = self.inner.get(key, range).await?;
        if key.contains("/seg/") && self.short.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(b.slice(..b.len().saturating_sub(1)));
        }
        Ok(b)
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.inner.delete(key).await
    }
}

/// A served volume whose block 0 is committed and store-resident, behind
/// a [`ShortBodies`] store.
async fn short_body_server(
    max_in_flight: usize,
) -> (TestServer, Arc<std::sync::atomic::AtomicBool>) {
    let short = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store: Arc<dyn ObjectStore> = Arc::new(ShortBodies {
        inner: Arc::new(MemoryStore::new()),
        short: short.clone(),
    });
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::new());
    let cfg = EngineConfig::default();
    let mut engine: DynEngine = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol,
        VOL_BYTES,
        cfg.clone(),
    )
    .await
    .unwrap();
    engine.write(0, &[0x11; 4096]).unwrap();
    engine
        .commit(skynbd_format::CommitKind::Flush)
        .await
        .unwrap();
    drop(engine);
    let engine: DynEngine = Engine::open(Arc::clone(&store), Arc::clone(&refs), vol, cfg)
        .await
        .unwrap();
    let registry = Arc::new(ExportRegistry::new());
    registry
        .register(
            "vol",
            Export {
                engine: Arc::new(tokio::sync::RwLock::new(engine)),
                read_only: false,
                size: VOL_BYTES,
            },
        )
        .await;
    let srv = TestServer::serve_with(registry, store, refs, vol, max_in_flight).await;
    (srv, short)
}

const DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// A served export whose two committed blocks (at 0 and 64 KiB, in
/// different segments) live only in a store that, once `armed`, delays
/// every GET by `DELAY`. 128 KiB onward is a hole: reading it is instant.
async fn slow_store_server(max_in_flight: usize) -> (TestServer, Arc<SlowGate>) {
    slow_store_server_with(max_in_flight, 1 << 30, true).await
}

/// [`slow_store_server`] with the shard size and warm-up chosen: with
/// 64 KiB shards the two blocks sit in different shards, and without the
/// warm-up each shard's first touch is a (delayed) node fetch.
async fn slow_store_server_with(
    max_in_flight: usize,
    shard_bytes: u64,
    warm: bool,
) -> (TestServer, Arc<SlowGate>) {
    let gate = Arc::new(SlowGate::default());
    let store: Arc<dyn ObjectStore> = Arc::new(SlowGets {
        inner: Arc::new(MemoryStore::new()),
        delay: DELAY,
        gate: gate.clone(),
    });
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::new());
    // A tiny segment size so the two blocks land in different segments:
    // each read is its own store GET.
    let cfg = EngineConfig {
        segment_max_bytes: 4096,
        shard_bytes,
        ..EngineConfig::default()
    };
    let mut engine: DynEngine = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol,
        VOL_BYTES,
        cfg.clone(),
    )
    .await
    .unwrap();
    engine.write(0, &[0xA1; 4096]).unwrap();
    engine.write(64 * 1024, &[0xB2; 4096]).unwrap();
    engine
        .commit(skynbd_format::CommitKind::Flush)
        .await
        .unwrap();
    // Reopen so nothing is served from the writer's in-memory segments,
    // and (when `warm`) materialize the index so each measured read is
    // exactly one data GET.
    drop(engine);
    let engine: DynEngine = Engine::open(Arc::clone(&store), Arc::clone(&refs), vol, cfg)
        .await
        .unwrap();
    if warm {
        engine.block_status(0, VOL_BYTES).await.unwrap();
    }
    let registry = Arc::new(ExportRegistry::new());
    registry
        .register(
            "vol",
            Export {
                engine: Arc::new(tokio::sync::RwLock::new(engine)),
                read_only: false,
                size: VOL_BYTES,
            },
        )
        .await;
    let srv = TestServer::serve_with(registry, store, refs, vol, max_in_flight).await;
    (srv, gate)
}

const HOLE: u64 = 128 * 1024;

impl Client {
    /// Send a request without waiting for its reply (pipelining).
    async fn send(&mut self, cmd: u16, offset: u64, len: u32, payload: &[u8]) -> u64 {
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        let mut hdr = [0u8; 28];
        hdr[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
        hdr[6..8].copy_from_slice(&cmd.to_be_bytes());
        hdr[8..16].copy_from_slice(&cookie.to_be_bytes());
        hdr[16..24].copy_from_slice(&offset.to_be_bytes());
        hdr[24..28].copy_from_slice(&len.to_be_bytes());
        self.stream.write_all(&hdr).await.unwrap();
        self.stream.write_all(payload).await.unwrap();
        cookie
    }

    /// Next simple reply, in wire order: (cookie, error, read payload).
    /// `read_len` says how many payload bytes follow a READ's reply.
    async fn recv(&mut self, read_len: impl Fn(u64) -> u32) -> (u64, u32, Vec<u8>) {
        assert_eq!(self.stream.read_u32().await.unwrap(), SIMPLE_REPLY_MAGIC);
        let err = self.stream.read_u32().await.unwrap();
        let cookie = self.stream.read_u64().await.unwrap();
        let mut data = vec![0u8; if err == 0 { read_len(cookie) } else { 0 } as usize];
        self.stream.read_exact(&mut data).await.unwrap();
        (cookie, err, data)
    }
}

/// Issue #97 item 2: a cold read waiting on the object store no longer
/// holds the export exclusively. Two connections each read a different
/// committed (store-resident) block while every store GET takes `DELAY`:
/// under the old per-export mutex they serialized (≥ 2 × DELAY); under
/// the shared read lock they overlap (~1 × DELAY). Writes still exclude.
#[tokio::test(flavor = "multi_thread")]
async fn cold_reads_on_one_export_do_not_serialize() {
    let (srv, gate) = slow_store_server(1).await;
    let (mut a, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let (mut b, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::Instant::now();
    let (ra, rb) = tokio::join!(a.read(4096 - 512, 512), b.read(64 * 1024, 512));
    let wall = t.elapsed();
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(ra.unwrap(), vec![0xA1; 512]);
    assert_eq!(rb.unwrap(), vec![0xB2; 512]);
    assert!(
        wall < DELAY * 2 - DELAY / 4,
        "two cold reads on one export serialized: {wall:?} for a {DELAY:?} store"
    );
}

/// BLOCK_STATUS shares the lock too: two connections each query a
/// different not-yet-materialized shard, whose first touch is a delayed
/// node fetch, and the two overlap.
#[tokio::test(flavor = "multi_thread")]
async fn cold_block_status_on_one_export_do_not_serialize() {
    let (srv, gate) = slow_store_server_with(16, 64 * 1024, false).await;
    let mut a = connect_structured(srv.addr, "vol").await;
    let mut b = connect_structured(srv.addr, "vol").await;
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::Instant::now();
    send_request(&mut a, CMD_BLOCK_STATUS, 0, 0, 4096).await;
    send_request(&mut b, CMD_BLOCK_STATUS, 0, 64 * 1024, 4096).await;
    let (ra, rb) = tokio::join!(read_structured(&mut a), read_structured(&mut b));
    let wall = t.elapsed();
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    for (_, rtype, data) in [ra, rb] {
        assert_eq!(rtype, REPLY_TYPE_BLOCK_STATUS);
        // One descriptor: 4 KiB allocated.
        assert_eq!(&data[4..12], &[0, 0, 0x10, 0, 0, 0, 0, 0]);
    }
    assert!(
        wall >= DELAY && wall < DELAY * 2 - DELAY / 4,
        "cold BLOCK_STATUS serialized (or was not cold): {wall:?} for a {DELAY:?} store"
    );
}

/// Writes still exclude reads: a WRITE sent while another connection's
/// cold READ is in flight completes only after that READ's delayed fetch.
/// The WRITE is sent once the READ's GET has started (signalled by the
/// store, not guessed with a sleep), so the READ holds the lock first.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_waits_for_an_in_flight_cold_read() {
    let (srv, gate) = slow_store_server(16).await;
    let (mut a, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let (mut b, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let started = gate.started.notified();
    tokio::pin!(started);
    started.as_mut().enable();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::Instant::now();
    let read = a.read(0, 512);
    let write = async {
        started.await; // the READ's cold GET is in flight, under its guard
        (b.write(0, &[0xCC; 4096]).await, t.elapsed())
    };
    let (r, (w, write_done)) = tokio::join!(read, write);
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(r.unwrap().len(), 512);
    assert_eq!(w, 0);
    assert!(
        write_done >= DELAY,
        "the write finished at {write_done:?}, inside the read's {DELAY:?} cold fetch"
    );
    assert_eq!(b.read(0, 512).await.unwrap(), vec![0xCC; 512]);
}

/// A request that will be refused takes no engine guard: a misaligned
/// WRITE (and a READ past the end) sent while another connection's cold
/// READ holds the lock is answered EINVAL at once, not after the fetch.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_request_does_not_wait_for_the_engine_lock() {
    let (srv, gate) = slow_store_server(16).await;
    let (mut a, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let (mut b, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let started = gate.started.notified();
    tokio::pin!(started);
    started.as_mut().enable();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let read = a.read(0, 512);
    let refused = async {
        started.await; // the cold READ holds its shared guard
        let t = std::time::Instant::now();
        let w = b.write(1, &[0xEE; 512]).await;
        // A WRITE queued for the exclusive lock would hold this READ too.
        let r = b.read(VOL_BYTES, 512).await;
        (w, r, t.elapsed())
    };
    let (cold, (w, r, took)) = tokio::join!(read, refused);
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(cold.unwrap().len(), 512);
    assert_eq!(w, 22, "misaligned WRITE");
    assert!(r.is_err(), "READ past the end");
    assert!(
        took < DELAY / 2,
        "refusals waited {took:?} behind a {DELAY:?} cold read"
    );
}

/// Issue #97 item 3: ONE connection pipelining two cold reads gets them
/// serviced concurrently — and a hole read sent after a cold one replies
/// first, out of order, matched by cookie.
#[tokio::test(flavor = "multi_thread")]
async fn one_connection_services_pipelined_requests_concurrently() {
    let (srv, gate) = slow_store_server(16).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::Instant::now();
    let cold_a = c.send(CMD_READ, 0, 512, &[]).await;
    let cold_b = c.send(CMD_READ, 64 * 1024, 512, &[]).await;
    let hole = c.send(CMD_READ, HOLE, 512, &[]).await;
    let mut order = Vec::new();
    for _ in 0..3 {
        let (cookie, err, data) = c.recv(|_| 512).await;
        assert_eq!(err, 0);
        let want = if cookie == cold_a {
            0xA1
        } else if cookie == cold_b {
            0xB2
        } else {
            0
        };
        assert_eq!(data, vec![want; 512], "cookie {cookie}");
        order.push(cookie);
    }
    let wall = t.elapsed();
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        order[0], hole,
        "the instant request replies first: {order:?}"
    );
    assert!(
        wall < DELAY * 2 - DELAY / 4,
        "pipelined cold reads serialized: {wall:?} for a {DELAY:?} store"
    );
}

/// The default (1) is the historical loop: the same pipelined requests
/// are serviced one at a time and reply in the order they were sent.
#[tokio::test(flavor = "multi_thread")]
async fn max_in_flight_one_stays_serial_and_in_order() {
    let (srv, gate) = slow_store_server(1).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let t = std::time::Instant::now();
    let sent = [
        c.send(CMD_READ, 0, 512, &[]).await,
        c.send(CMD_READ, 64 * 1024, 512, &[]).await,
        c.send(CMD_READ, HOLE, 512, &[]).await,
    ];
    let mut order = Vec::new();
    for _ in 0..3 {
        order.push(c.recv(|_| 512).await.0);
    }
    let wall = t.elapsed();
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(order, sent);
    assert!(wall >= DELAY * 2, "depth 1 overlapped: {wall:?}");
}

/// FLUSH waits for writes, not reads: on a clean volume a FLUSH sent
/// behind a slow cold READ is answered at once, under the shared lock,
/// and the READ replies after it with the right bytes.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_flush_does_not_wait_for_a_slow_read() {
    let (srv, gate) = slow_store_server(16).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let read = c.send(CMD_READ, 0, 512, &[]).await;
    let flush = c.send(CMD_FLUSH, 0, 0, &[]).await;
    let first = c.recv(|ck| if ck == read { 512 } else { 0 }).await;
    let second = c.recv(|ck| if ck == read { 512 } else { 0 }).await;
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!((first.0, first.1), (flush, 0), "flush waited for the read");
    assert_eq!((second.0, second.1, second.2), (read, 0, vec![0xA1; 512]));
}

/// Overlapping requests apply in the order they were sent: each takes
/// its engine guard in the parse loop, and tokio's `RwLock` is FIFO.
/// Pipelined WRITE A, WRITE B, READ of the same block always reads B.
#[tokio::test(flavor = "multi_thread")]
async fn overlapping_pipelined_requests_apply_in_receive_order() {
    let srv = TestServer::start().await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    for round in 0..64u8 {
        let a = vec![round; 4096];
        let b = vec![round ^ 0xFF; 4096];
        let wa = c.send(CMD_WRITE, 0, 4096, &a).await;
        let wb = c.send(CMD_WRITE, 0, 4096, &b).await;
        let r = c.send(CMD_READ, 0, 4096, &[]).await;
        let mut got = None;
        for _ in 0..3 {
            let (cookie, err, data) = c.recv(|ck| if ck == r { 4096 } else { 0 }).await;
            assert_eq!(err, 0);
            assert!([wa, wb, r].contains(&cookie));
            if cookie == r {
                got = Some(data);
            }
        }
        assert_eq!(got.unwrap(), b, "round {round}: B did not land last");
    }
}

/// DISC drains: a slow READ pipelined before DISC still gets its reply
/// before the connection closes.
#[tokio::test(flavor = "multi_thread")]
async fn disc_answers_every_request_before_it() {
    let (srv, gate) = slow_store_server(16).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let read = c.send(CMD_READ, 0, 512, &[]).await;
    c.send(CMD_DISC, 0, 0, &[]).await;
    let (cookie, err, data) = c.recv(|_| 512).await;
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!((cookie, err, data), (read, 0, vec![0xA1; 512]));
    let mut rest = Vec::new();
    c.stream.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty(), "bytes after the last reply: {rest:?}");
}

/// DISC's courtesy commit covers writes pipelined before it: with no
/// FLUSH at all, a WRITE sent just ahead of DISC survives a kill -9
/// taken once the connection has closed.
#[tokio::test(flavor = "multi_thread")]
async fn disc_commits_writes_pipelined_before_it() {
    let srv = TestServer::start().await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let block = vec![0x6D; 4096];
    let w = c.send(CMD_WRITE, 8192, 4096, &block).await;
    c.send(CMD_DISC, 0, 0, &[]).await;
    assert_eq!(c.recv(|_| 0).await.0, w);
    let mut rest = Vec::new();
    c.stream.read_to_end(&mut rest).await.unwrap(); // closed: commit done
    let (store, refs, vol) = srv.kill_dash_nine().await;
    let srv = TestServer::restart(store, refs, vol).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    assert_eq!(c.read(8192, 4096).await.unwrap(), block);
}

/// Structured replies pipeline too, errors included: a cold READ, a READ
/// past the end (a structured error chunk) and a BLOCK_STATUS of the hole
/// are in flight together; the two fast ones answer first, each frame
/// whole and matched by cookie.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_structured_replies_and_errors_stay_framed() {
    let (srv, gate) = slow_store_server(16).await;
    let mut st = connect_structured(srv.addr, "vol").await;
    gate.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    send_request_c(&mut st, CMD_READ, 0, 64 * 1024, 4096, 1).await;
    send_request_c(&mut st, CMD_READ, 0, VOL_BYTES, 512, 2).await;
    send_request_c(&mut st, CMD_BLOCK_STATUS, 0, HOLE, 4096, 3).await;
    let mut got = std::collections::BTreeMap::new();
    let mut order = Vec::new();
    for _ in 0..3 {
        let (cookie, flags, rtype, data) = read_structured_c(&mut st).await;
        assert_eq!(flags & REPLY_FLAG_DONE, REPLY_FLAG_DONE);
        order.push(cookie);
        got.insert(cookie, (rtype, data));
    }
    gate.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(order[2], 1, "the cold READ answers last: {order:?}");
    let (rtype, data) = &got[&1];
    assert_eq!(*rtype, REPLY_TYPE_OFFSET_DATA);
    assert_eq!(
        u64::from_be_bytes(data[0..8].try_into().unwrap()),
        64 * 1024
    );
    assert_eq!(&data[8..], &[0xB2; 4096][..]);
    let (rtype, data) = &got[&2];
    assert_eq!(*rtype, REPLY_TYPE_ERROR);
    assert_eq!(u32::from_be_bytes(data[0..4].try_into().unwrap()), EINVAL);
    let (rtype, data) = &got[&3];
    assert_eq!(*rtype, REPLY_TYPE_BLOCK_STATUS);
    assert_eq!(
        u32::from_be_bytes(data[8..12].try_into().unwrap()),
        STATE_HOLE | STATE_ZERO
    );
}

/// A request task that panics (here: a store returning a short body,
/// which the engine's copy cannot absorb) must not leave its cookie
/// unanswered with the connection open: the client gets an error reply
/// or EOF promptly, at the default depth and pipelined alike.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_request_task_never_hangs_the_connection() {
    for max_in_flight in [1usize, 16] {
        let (srv, short) = short_body_server(max_in_flight).await;
        let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
        short.store(true, std::sync::atomic::Ordering::SeqCst);
        c.send(CMD_READ, 0, 4096, &[]).await;
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            match c.stream.read_u32().await {
                Err(_) => "eof",
                Ok(SIMPLE_REPLY_MAGIC) => {
                    let err = c.stream.read_u32().await.unwrap();
                    assert_ne!(err, 0, "a short body read as success");
                    "error reply"
                }
                Ok(m) => panic!("bad magic {m:#x}"),
            }
        })
        .await;
        assert!(outcome.is_ok(), "depth {max_in_flight}: connection hung");
    }
}

/// Pipelined writes, a flush, and pipelined reads back: every byte lands
/// and survives, with all of it in flight at once.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_writes_then_flush_are_durable_and_exact() {
    let srv = TestServer::start().await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let mut sent = Vec::new();
    for i in 0..32u64 {
        let block = vec![i as u8 ^ 0x5A; 4096];
        sent.push(c.send(CMD_WRITE, i * 4096, 4096, &block).await);
    }
    let flush = c.send(CMD_FLUSH, 0, 0, &[]).await;
    let mut acked = Vec::new();
    for _ in 0..33 {
        let (cookie, err, _) = c.recv(|_| 0).await;
        assert_eq!(err, 0);
        acked.push(cookie);
    }
    assert_eq!(
        *acked.last().unwrap(),
        flush,
        "flush replies after every write"
    );
    acked.pop();
    acked.sort_unstable();
    assert_eq!(acked, sent);

    // Survive a restart: the flush made all 32 writes durable. No DISC
    // (its courtesy commit would mask a FLUSH that covered nothing): the
    // connection is just dropped.
    drop(c);
    let (store, refs, vol) = srv.kill_dash_nine().await;
    let srv = TestServer::restart(store, refs, vol).await;
    let (mut c, _, _) = Client::connect(srv.addr, "vol").await.unwrap();
    let reads: Vec<u64> = {
        let mut v = Vec::new();
        for i in 0..32u64 {
            v.push(c.send(CMD_READ, i * 4096, 4096, &[]).await);
        }
        v
    };
    for _ in 0..32 {
        let (cookie, err, data) = c.recv(|_| 4096).await;
        assert_eq!(err, 0);
        let i = reads.iter().position(|&r| r == cookie).unwrap() as u8;
        assert_eq!(data, vec![i ^ 0x5A; 4096]);
    }
}
