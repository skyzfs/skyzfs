//! NBD server: export registry, negotiation, transmission (design §8).
//!
//! One tokio task per connection. Requests are parsed in order and
//! serviced concurrently, up to [`Server::max_in_flight`] per connection
//! (see `transmission`), over a shared per-volume engine `RwLock` (see
//! [`Export::engine`]); the message-passing writer task of design §7.3
//! lands with the M3+ registry work.

use std::collections::HashMap;
use std::sync::Arc;

use skynbd_engine::{Engine, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::RefStore;
use skynbd_store::ObjectStore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::RwLock;

use crate::acl::ConnAccess;
use crate::proto::*;

/// Type-erased engine so one registry serves any backend combination.
pub type DynEngine = Engine<Arc<dyn ObjectStore>, Arc<dyn RefStore>>;

#[derive(Clone)]
pub struct Export {
    /// Lock discipline (issue #97 item 2): `read()` for anything that only
    /// needs `&DynEngine` (READ, BLOCK_STATUS, head/seq lookups), so a cold
    /// read waiting on the object store no longer stalls every other read
    /// on the export; `write()` for writes, commits and maintenance. No
    /// guard is held while a reply goes to the socket, so a client that
    /// stops draining stalls only itself.
    ///
    /// GC and rollback take this engine's lock exclusively, so no reader
    /// *of this engine* sees an object they delete vanish mid-fetch.
    /// Exports of other engines on the same volume (`snap/*`, `rpo`,
    /// `origin`) have their own locks and get no such exclusion.
    ///
    /// Limit: the shared guard is held across a cold fetch, and tokio's
    /// `RwLock` is fair, so once a writer queues, later reads wait for
    /// the slowest in-flight read plus the write. Reads overlap freely
    /// only while no writer is queued (read-only exports always).
    pub engine: Arc<RwLock<DynEngine>>,
    pub read_only: bool,
    pub size: u64,
}

impl Export {
    pub fn tflags(&self) -> u16 {
        if self.read_only {
            TFLAG_HAS_FLAGS | TFLAG_READ_ONLY
        } else {
            TFLAG_HAS_FLAGS
                | TFLAG_SEND_FLUSH
                | TFLAG_SEND_FUA
                | TFLAG_SEND_TRIM
                | TFLAG_SEND_WRITE_ZEROES
        }
    }
}

/// Name → export. M2: mutated via API by the daemon; M3 derives this from
/// the ref store via watch events (design §3.1).
#[derive(Default)]
pub struct ExportRegistry {
    map: RwLock<HashMap<String, Export>>,
}

impl ExportRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, name: &str, export: Export) {
        self.map.write().await.insert(name.to_string(), export);
    }

    pub async fn unregister(&self, name: &str) {
        self.map.write().await.remove(name);
    }

    pub async fn get(&self, name: &str) -> Option<Export> {
        self.map.read().await.get(name).cloned()
    }

    pub async fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.map.read().await.keys().cloned().collect();
        v.sort();
        v
    }
}

pub struct Server {
    registry: Arc<ExportRegistry>,
    /// STARTTLS context (auth design §3.1): present = forced-TLS mode.
    tls: Option<Arc<crate::tls::TlsContext>>,
    /// Bound on the whole handshake (greeting through option haggling,
    /// including the TLS handshake) so half-open clients cannot pin
    /// connection tasks. Transmission is unbounded.
    negotiation_timeout: std::time::Duration,
    /// Requests serviced concurrently per connection (issue #97 item 3);
    /// 1 = one at a time, the historical behavior and the default.
    max_in_flight: usize,
}

/// Upper bound for [`Server::max_in_flight`]: far above any client queue
/// depth worth serving. Each in-flight WRITE holds its payload (up to
/// [`HARD_MAX_IO`]) until it applies, so this also bounds what one
/// connection can make the server buffer.
pub const MAX_IN_FLIGHT_LIMIT: usize = 1024;

impl Server {
    pub fn new(registry: Arc<ExportRegistry>) -> Self {
        Self {
            registry,
            tls: None,
            negotiation_timeout: std::time::Duration::from_secs(30),
            max_in_flight: 1,
        }
    }

    /// Service up to `n` requests concurrently per connection, replying
    /// as each finishes (out of order, matched by cookie — NBD permits
    /// it). Parsing stays serial, and requests reach the engine in the
    /// order they were sent, so overlapping ones apply that way. FLUSH
    /// waits for every write before it; DISC for every request. Clamped
    /// to `1..=`[`MAX_IN_FLIGHT_LIMIT`]. See `transmission`.
    ///
    /// Memory: each in-flight WRITE holds its payload until it replies,
    /// so a connection may buffer up to `n × HARD_MAX_IO` (32 MiB) —
    /// in practice `n ×` the client's max request size (the kernel
    /// client's is typically ≤ 1 MiB).
    pub fn max_in_flight(mut self, n: usize) -> Self {
        self.max_in_flight = n.clamp(1, MAX_IN_FLIGHT_LIMIT);
        self
    }

    /// Enable forced STARTTLS: pre-TLS, every option except
    /// `STARTTLS`/`ABORT` is answered `REP_ERR_TLS_REQD` (FORCEDTLS,
    /// design §3.1); post-handshake, the client-cert identity is matched
    /// against the context's ACL table on every export access.
    pub fn with_tls(mut self, tls: Arc<crate::tls::TlsContext>) -> Self {
        self.tls = Some(tls);
        self
    }

    pub fn negotiation_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.negotiation_timeout = timeout;
        self
    }

    /// Accept loop; spawn one task per connection. Runs until the
    /// listener errors or the task is aborted (daemon handles signals).
    pub async fn run(self, listener: TcpListener) -> std::io::Result<()> {
        loop {
            let (stream, peer) = listener.accept().await?;
            let registry = Arc::clone(&self.registry);
            let tls = self.tls.clone();
            let timeout = self.negotiation_timeout;
            let max_in_flight = self.max_in_flight;
            tokio::spawn(async move {
                stream.set_nodelay(true).ok();
                if let Err(e) =
                    handle_connection(stream, registry, tls, timeout, max_in_flight).await
                {
                    tracing::debug!(%peer, error = %e, "connection ended");
                }
            });
        }
    }

    /// Same accept loop over a Unix domain socket (design §15 M10a): the
    /// protocol is transport-agnostic; access control is the socket file
    /// mode instead of a TCP port (plus TLS/ACLs when configured).
    pub async fn run_unix(self, listener: UnixListener) -> std::io::Result<()> {
        loop {
            let (stream, _addr) = listener.accept().await?;
            let registry = Arc::clone(&self.registry);
            let tls = self.tls.clone();
            let timeout = self.negotiation_timeout;
            let max_in_flight = self.max_in_flight;
            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(stream, registry, tls, timeout, max_in_flight).await
                {
                    tracing::debug!(error = %e, "unix connection ended");
                }
            });
        }
    }
}

/// Any bidirectional byte stream can carry NBD (TCP, UDS, tests over
/// duplex pipes).
pub trait NbdStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> NbdStream for T {}

/// Plain or TLS-upgraded transport for the post-negotiation phases: the
/// stream type changes mid-connection at STARTTLS, so the option loop
/// and transmission run over this enum.
enum MaybeTls<St> {
    Plain(St),
    Tls(Box<tokio_rustls::server::TlsStream<St>>),
}

impl<St: NbdStream> AsyncRead for MaybeTls<St> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl<St: NbdStream> AsyncWrite for MaybeTls<St> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Per-connection negotiated capabilities (design §8.1, M4).
#[derive(Default, Clone, Copy)]
struct ConnState {
    structured: bool,
    /// base:allocation selected via SET_META_CONTEXT.
    allocation_ctx: bool,
}

enum Negotiated<St> {
    Serve(St, Export, ConnState),
    Closed,
}

async fn handle_connection<St: NbdStream + 'static>(
    stream: St,
    registry: Arc<ExportRegistry>,
    tls: Option<Arc<crate::tls::TlsContext>>,
    timeout: std::time::Duration,
    max_in_flight: usize,
) -> std::io::Result<()> {
    let negotiated = tokio::time::timeout(timeout, negotiate(stream, &registry, tls.as_deref()))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "negotiation timeout"))??;
    match negotiated {
        Negotiated::Serve(stream, export, conn) => {
            transmission(stream, export, conn, max_in_flight).await
        }
        Negotiated::Closed => Ok(()),
    }
}

// ------------------------------------------------------------ negotiation

async fn negotiate<St: NbdStream>(
    mut stream: St,
    registry: &ExportRegistry,
    tls: Option<&crate::tls::TlsContext>,
) -> std::io::Result<Negotiated<MaybeTls<St>>> {
    // Greeting: NBDMAGIC + IHAVEOPT + handshake flags (fixed newstyle).
    let mut greeting = [0u8; 18];
    greeting[0..8].copy_from_slice(&NBDMAGIC.to_be_bytes());
    greeting[8..16].copy_from_slice(&IHAVEOPT.to_be_bytes());
    greeting[16..18].copy_from_slice(&(FLAG_FIXED_NEWSTYLE | FLAG_NO_ZEROES).to_be_bytes());
    stream.write_all(&greeting).await?;

    let client_flags = stream.read_u32().await?;
    if client_flags & CFLAG_FIXED_NEWSTYLE == 0 {
        return Ok(Negotiated::Closed); // oldstyle clients unsupported
    }
    let client_no_zeroes = client_flags & CFLAG_NO_ZEROES != 0;

    match tls {
        None => options_loop(MaybeTls::Plain(stream), registry, None, client_no_zeroes).await,
        Some(ctx) => {
            // FORCEDTLS pre-TLS phase: nothing but STARTTLS/ABORT is
            // productive, so export names never cross in cleartext.
            match pretls_loop(&mut stream).await? {
                PreTls::Closed => return Ok(Negotiated::Closed),
                PreTls::Upgrade => {}
            }
            let tls_stream = ctx.acceptor().await.accept(stream).await?;
            let identity = {
                let (_, conn) = tls_stream.get_ref();
                conn.peer_certificates()
                    .and_then(|certs| certs.first())
                    .and_then(crate::tls::peer_identity)
                    .ok_or_else(|| {
                        std::io::Error::other("client certificate carries no usable identity")
                    })?
            };
            let access = ConnAccess::new(identity, Arc::clone(&ctx.acl));
            options_loop(
                MaybeTls::Tls(Box::new(tls_stream)),
                registry,
                Some(access),
                client_no_zeroes,
            )
            .await
        }
    }
}

enum PreTls {
    Upgrade,
    Closed,
}

/// Forced-TLS option loop before the handshake: `STARTTLS` (empty
/// payload) upgrades, `ABORT` closes, everything else is consumed and
/// answered `REP_ERR_TLS_REQD` (`EXPORT_NAME` has no error-reply path in
/// the protocol, so it closes).
async fn pretls_loop<St: NbdStream>(stream: &mut St) -> std::io::Result<PreTls> {
    loop {
        let magic = stream.read_u64().await?;
        if magic != IHAVEOPT {
            return Ok(PreTls::Closed);
        }
        let option = stream.read_u32().await?;
        let len = stream.read_u32().await?;
        if len > MAX_OPT_LEN {
            return Ok(PreTls::Closed); // refuse to buffer; drop
        }
        let mut payload = vec![0u8; len as usize];
        stream.read_exact(&mut payload).await?;
        match option {
            OPT_STARTTLS if payload.is_empty() => {
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
                return Ok(PreTls::Upgrade);
            }
            OPT_STARTTLS => {
                stream
                    .write_all(&opt_reply_header(option, REP_ERR_INVALID, 0))
                    .await?;
            }
            OPT_ABORT => {
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
                return Ok(PreTls::Closed);
            }
            OPT_EXPORT_NAME => return Ok(PreTls::Closed),
            _ => {
                stream
                    .write_all(&opt_reply_header(option, REP_ERR_TLS_REQD, 0))
                    .await?;
            }
        }
    }
}

/// Resolve an export through the connection's ACL (auth design §3.3):
/// `None` both for nonexistent and denied (no existence oracle); a
/// read-only grant forces the export read-only for this connection.
async fn resolve_export(
    registry: &ExportRegistry,
    access: Option<&ConnAccess>,
    name: &str,
) -> Option<Export> {
    let export = registry.get(name).await?;
    match access {
        None => Some(export),
        Some(a) => match a.access(name)? {
            true => Some(export),
            false => Some(Export {
                read_only: true,
                ..export
            }),
        },
    }
}

async fn options_loop<St: NbdStream>(
    mut stream: St,
    registry: &ExportRegistry,
    access: Option<ConnAccess>,
    client_no_zeroes: bool,
) -> std::io::Result<Negotiated<St>> {
    let access = access.as_ref();
    let mut conn = ConnState::default();

    loop {
        let magic = stream.read_u64().await?;
        if magic != IHAVEOPT {
            return Ok(Negotiated::Closed);
        }
        let option = stream.read_u32().await?;
        let len = stream.read_u32().await?;
        if len > MAX_OPT_LEN {
            return Ok(Negotiated::Closed); // refuse to buffer; drop
        }
        let mut payload = vec![0u8; len as usize];
        stream.read_exact(&mut payload).await?;

        match option {
            OPT_LIST => {
                // ACL-filtered (design §3.3): a client only ever learns of
                // exports it may access.
                for name in registry.names().await {
                    if let Some(a) = access {
                        if a.access(&name).is_none() {
                            continue;
                        }
                    }
                    let hdr = opt_reply_header(option, REP_SERVER, 4 + name.len() as u32);
                    stream.write_all(&hdr).await?;
                    stream.write_all(&(name.len() as u32).to_be_bytes()).await?;
                    stream.write_all(name.as_bytes()).await?;
                }
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
            }
            OPT_GO | OPT_INFO => {
                let Some((name, _info_reqs)) = parse_go_payload(&payload) else {
                    stream
                        .write_all(&opt_reply_header(option, REP_ERR_INVALID, 0))
                        .await?;
                    continue;
                };
                let Some(export) = resolve_export(registry, access, &name).await else {
                    stream
                        .write_all(&opt_reply_header(option, REP_ERR_UNKNOWN, 0))
                        .await?;
                    continue;
                };
                let ie = info_export(export.size, export.tflags());
                stream
                    .write_all(&opt_reply_header(option, REP_INFO, ie.len() as u32))
                    .await?;
                stream.write_all(&ie).await?;
                let ib = info_block_size();
                stream
                    .write_all(&opt_reply_header(option, REP_INFO, ib.len() as u32))
                    .await?;
                stream.write_all(&ib).await?;
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
                if option == OPT_GO {
                    if let Some(a) = access {
                        tracing::info!(identity = %a.identity, export = %name, "attached");
                    }
                    return Ok(Negotiated::Serve(stream, export, conn));
                }
            }
            OPT_EXPORT_NAME => {
                // Legacy path: no error reply exists; unknown/denied → close.
                let Ok(name) = String::from_utf8(payload) else {
                    return Ok(Negotiated::Closed);
                };
                let Some(export) = resolve_export(registry, access, &name).await else {
                    return Ok(Negotiated::Closed);
                };
                stream.write_all(&export.size.to_be_bytes()).await?;
                stream.write_all(&export.tflags().to_be_bytes()).await?;
                if !client_no_zeroes {
                    stream.write_all(&[0u8; 124]).await?;
                }
                if let Some(a) = access {
                    tracing::info!(identity = %a.identity, export = %name, "attached (legacy)");
                }
                return Ok(Negotiated::Serve(stream, export, conn));
            }
            OPT_ABORT => {
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
                return Ok(Negotiated::Closed);
            }
            OPT_STARTTLS => {
                // Already upgraded (access set) → INVALID per spec; TLS
                // not configured → UNSUP, matching pre-STARTTLS behavior.
                let rep = if access.is_some() {
                    REP_ERR_INVALID
                } else {
                    REP_ERR_UNSUP
                };
                stream.write_all(&opt_reply_header(option, rep, 0)).await?;
            }
            OPT_STRUCTURED_REPLY => {
                if payload.is_empty() {
                    conn.structured = true;
                    stream
                        .write_all(&opt_reply_header(option, REP_ACK, 0))
                        .await?;
                } else {
                    stream
                        .write_all(&opt_reply_header(option, REP_ERR_INVALID, 0))
                        .await?;
                }
            }
            OPT_SET_META_CONTEXT | OPT_LIST_META_CONTEXT => {
                // SET requires structured replies first (spec).
                if option == OPT_SET_META_CONTEXT && !conn.structured {
                    stream
                        .write_all(&opt_reply_header(option, REP_ERR_INVALID, 0))
                        .await?;
                    continue;
                }
                let Some(queries) = parse_meta_context_payload(&payload) else {
                    stream
                        .write_all(&opt_reply_header(option, REP_ERR_INVALID, 0))
                        .await?;
                    continue;
                };
                // Empty SET query list resets selections (spec).
                if option == OPT_SET_META_CONTEXT && queries.is_empty() {
                    conn.allocation_ctx = false;
                }
                let matches_allocation = queries.is_empty() && option == OPT_LIST_META_CONTEXT
                    || queries.iter().any(|q| q == BASE_ALLOCATION || q == "base:");
                if matches_allocation {
                    let payload_len = 4 + BASE_ALLOCATION.len() as u32;
                    stream
                        .write_all(&opt_reply_header(option, REP_META_CONTEXT, payload_len))
                        .await?;
                    stream.write_all(&ALLOCATION_CTX_ID.to_be_bytes()).await?;
                    stream.write_all(BASE_ALLOCATION.as_bytes()).await?;
                    if option == OPT_SET_META_CONTEXT {
                        conn.allocation_ctx = true;
                    }
                }
                stream
                    .write_all(&opt_reply_header(option, REP_ACK, 0))
                    .await?;
            }
            _ => {
                stream
                    .write_all(&opt_reply_header(option, REP_ERR_UNSUP, 0))
                    .await?;
            }
        }
    }
}

/// GO/INFO payload: u32 name_len, name, u16 n_info_reqs, u16 × n.
fn parse_go_payload(p: &[u8]) -> Option<(String, Vec<u16>)> {
    if p.len() < 6 {
        return None;
    }
    let name_len = u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize;
    let name_end = 4usize.checked_add(name_len)?;
    if p.len() < name_end + 2 {
        return None;
    }
    let name = String::from_utf8(p[4..name_end].to_vec()).ok()?;
    let n = u16::from_be_bytes(p[name_end..name_end + 2].try_into().unwrap()) as usize;
    let reqs_end = name_end + 2 + n * 2;
    if p.len() < reqs_end {
        return None;
    }
    let reqs = p[name_end + 2..reqs_end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    Some((name, reqs))
}

/// SET/LIST_META_CONTEXT payload: u32 export_name_len, name, u32 n_queries,
/// then n × (u32 len, bytes). Returns the query strings.
fn parse_meta_context_payload(p: &[u8]) -> Option<Vec<String>> {
    if p.len() < 8 {
        return None;
    }
    let name_len = u32::from_be_bytes(p[0..4].try_into().unwrap()) as usize;
    let mut pos = 4usize.checked_add(name_len)?;
    if p.len() < pos + 4 {
        return None;
    }
    let n = u32::from_be_bytes(p[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    if n > 64 {
        return None;
    }
    let mut queries = Vec::with_capacity(n);
    for _ in 0..n {
        if p.len() < pos + 4 {
            return None;
        }
        let qlen = u32::from_be_bytes(p[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if p.len() < pos + qlen {
            return None;
        }
        queries.push(String::from_utf8(p[pos..pos + qlen].to_vec()).ok()?);
        pos += qlen;
    }
    Some(queries)
}

// ----------------------------------------------------------- transmission

/// The connection's write half, shared by every in-flight request. A
/// reply (header plus any payload) is written whole and flushed while
/// holding it, so concurrent replies never interleave on the wire; NBD
/// matches them to requests by cookie, so their ORDER is free (issue #97
/// item 3).
type Replies<St> = Arc<tokio::sync::Mutex<Wire<St>>>;

/// The write half, and whether a reply on it was cut off part-way (its
/// task cancelled or its write failed mid-frame). After that the stream
/// is out of frame: a later reply would land inside the truncated one's
/// payload, where the client would take it as data. So every later send
/// fails instead, and the connection ends with nothing more written.
struct Wire<St> {
    w: tokio::io::WriteHalf<St>,
    poisoned: bool,
}

/// In-flight request tasks of one class.
type Tasks = tokio::task::JoinSet<std::io::Result<()>>;

/// Write one reply's parts back to back and flush, under the reply lock.
/// The flush matters over TLS: a reply's tail can sit in the session's
/// buffer after `write_all` returns, and nothing else would push it out
/// if no later reply follows.
async fn send<St: NbdStream>(wr: &Replies<St>, parts: &[&[u8]]) -> std::io::Result<()> {
    let mut wire = wr.lock().await;
    if wire.poisoned {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "reply stream out of frame: an earlier reply was cut off",
        ));
    }
    // Pessimistically poisoned for the duration: a cancellation (or an
    // error) between here and the end leaves it set.
    wire.poisoned = true;
    for p in parts {
        wire.w.write_all(p).await?;
    }
    wire.w.flush().await?;
    wire.poisoned = false;
    Ok(())
}

/// Error reply in whichever framing the connection negotiated. READ and
/// BLOCK_STATUS errors must be structured once structured replies are on;
/// we use structured errors for every command then, which is permitted.
async fn send_err<St: NbdStream>(
    wr: &Replies<St>,
    conn: ConnState,
    cookie: u64,
    err: u32,
) -> std::io::Result<()> {
    if conn.structured {
        let hdr = structured_reply(REPLY_FLAG_DONE, REPLY_TYPE_ERROR, cookie, 6);
        send(wr, &[&hdr, &structured_error_payload(err)]).await
    } else {
        send(wr, &[&simple_reply(err, cookie)]).await
    }
}

/// A finished task's outcome: its own error, or its panic. A task that
/// was aborted (hang-up, below) is not a failure.
fn outcome(done: Result<std::io::Result<()>, tokio::task::JoinError>) -> std::io::Result<()> {
    match done {
        Ok(r) => r,
        Err(e) if e.is_cancelled() => Ok(()),
        Err(e) => Err(std::io::Error::other(e)),
    }
}

/// Reap every finished task, then wait until at most `keep` remain in
/// flight. The first failure ends the connection.
async fn settle(reads: &mut Tasks, writes: &mut Tasks, keep: usize) -> std::io::Result<()> {
    loop {
        while let Some(done) = reads.try_join_next().or_else(|| writes.try_join_next()) {
            outcome(done)?;
        }
        if reads.len() + writes.len() <= keep {
            return Ok(());
        }
        let done = tokio::select! {
            Some(d) = reads.join_next(), if !reads.is_empty() => d,
            Some(d) = writes.join_next(), if !writes.is_empty() => d,
        };
        outcome(done)?;
    }
}

/// Wait for EVERY task of one class, returning the first failure. A
/// failure must not stop the join: the caller drops the set next, which
/// would abort the tasks still running — a write mid-apply or mid-commit.
async fn drain(tasks: &mut Tasks) -> std::io::Result<()> {
    let mut first = Ok(());
    while let Some(done) = tasks.join_next().await {
        if let (Err(e), true) = (outcome(done), first.is_ok()) {
            first = Err(e);
        }
    }
    first
}

/// Start a request. A panic in it shuts the connection down rather than
/// leaving its cookie unanswered while the parse loop waits for a header
/// the client will never send: the client sees EOF and fails the IO at
/// once, as it did when a panic killed the serial loop.
fn start_request<St, F>(tasks: &mut Tasks, wr: &Replies<St>, fut: F)
where
    St: NbdStream + 'static,
    F: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
{
    use futures_util::FutureExt;
    let wr = Arc::clone(wr);
    start(tasks, async move {
        match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
            Ok(r) => r,
            Err(_) => {
                let _ = wr.lock().await.w.shutdown().await;
                Err(std::io::Error::other("request task panicked"))
            }
        }
    });
}

/// Run a request here, in the parse loop, until it first has to wait —
/// on the store, a FUA commit, the reply lock, or a full socket — and
/// only then give it a task of its own. A request that never waits (a
/// cached read, a write, a refusal) is answered with no spawn at all, as
/// the serial loop answered it: a task per request costs a spawn and
/// cross-worker wakeups that dominate a request this cheap, so spawning
/// every one made QD1 slower and hot QD32 about half as fast.
/// Concurrency is only worth that for a request that waits, and those
/// still get it.
///
/// The first poll uses a no-op waker. That's sound: a pending request is
/// spawned at once, and the task's first poll, which the runtime makes
/// promptly, registers its real waker before anything can be missed.
fn start<F>(tasks: &mut Tasks, fut: F)
where
    F: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
{
    use std::task::{Context, Poll, Waker};
    let mut fut = Box::pin(fut);
    match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(())) => {}
        // Reported by the next `settle`, like a task's failure.
        Poll::Ready(Err(e)) => {
            tasks.spawn(std::future::ready(Err(e)));
        }
        Poll::Pending => {
            tasks.spawn(fut);
        }
    }
}

/// The transmission phase (issue #97 item 3). Parsing stays strictly
/// serial — a WRITE's payload follows its header inline, so the next
/// header cannot be read before it — but servicing does not: a request
/// that has to wait continues in its own task ([`start`]), up to
/// `max_in_flight` at once, and replies as it finishes. One that never
/// waits is answered inline. Finished tasks are reaped, and a slot
/// awaited, BEFORE the next header is read, so at `max_in_flight == 1`
/// this is exactly the historical loop: read, service, reply, then read
/// the next request.
///
/// **Receive order.** Each request takes its engine guard here, in the
/// parse loop, before it starts: `read_owned` for READ and
/// BLOCK_STATUS, `write_owned` for WRITE, WRITE_ZEROES and TRIM. tokio's
/// `RwLock` is FIFO, so requests reach the engine in the order they were
/// sent, and overlapping ones apply that way (WRITE→WRITE, WRITE→READ).
/// NBD would permit reordering; nothing here relies on it. A request
/// that will be refused (misaligned, out of range, a write to a
/// read-only export) touches no engine state, so it is answered without
/// taking a guard — an EINVAL never waits for another request's IO.
///
/// **FLUSH** waits for every write-class request before it (their applies
/// and replies), then commits. Reads before it are not waited for as
/// such, but a commit needs the exclusive lock, so it does wait for any
/// in-flight read's *engine call*. A FLUSH with nothing uncommitted is
/// answered under the shared lock, so it waits for no read of its own —
/// unless another connection's writer is already queued for the
/// exclusive lock, which the fair lock makes it wait behind.
///
/// **DISC** drains everything, so every reply is out, then runs the
/// courtesy commit. On hang-up or a desynced stream, in-flight reads are
/// aborted (their replies can't be delivered) and every write is let
/// finish, whichever task failed first. A read aborted mid-reply leaves
/// the stream poisoned ([`Wire`]), so no later reply is appended to its
/// truncated frame.
async fn transmission<St: NbdStream + 'static>(
    stream: St,
    export: Export,
    conn: ConnState,
    max_in_flight: usize,
) -> std::io::Result<()> {
    let max_in_flight = max_in_flight.max(1);
    let (mut rd, wr) = tokio::io::split(stream);
    let wr: Replies<St> = Arc::new(tokio::sync::Mutex::new(Wire {
        w: wr,
        poisoned: false,
    }));
    let (mut reads, mut writes) = (Tasks::new(), Tasks::new());
    let mut hdr = [0u8; Request::WIRE_LEN];
    let result = loop {
        if let Err(e) = settle(&mut reads, &mut writes, max_in_flight - 1).await {
            break Err(e);
        }
        if rd.read_exact(&mut hdr).await.is_err() {
            break Ok(()); // client hung up
        }
        let req = match Request::decode(&hdr) {
            Ok(r) => r,
            Err(_) => break Ok(()), // desynced stream: drop connection
        };
        // A WRITE's payload follows its header: consume it whatever the
        // outcome, so the stream stays in frame.
        let data = if req.cmd == CMD_WRITE {
            if req.len > HARD_MAX_IO {
                break Ok(()); // refuse to buffer; drop connection
            }
            let mut data = vec![0u8; req.len as usize];
            if let Err(e) = rd.read_exact(&mut data).await {
                break Err(e);
            }
            data
        } else {
            Vec::new()
        };
        if let Some(err) = refusal(&export, conn, &req) {
            let w = Arc::clone(&wr);
            start_request(&mut reads, &wr, async move {
                if matches!(req.cmd, CMD_READ | CMD_BLOCK_STATUS) {
                    send_err(&w, conn, req.cookie, err).await
                } else {
                    send(&w, &[&simple_reply(err, req.cookie)]).await
                }
            });
            continue;
        }
        match req.cmd {
            CMD_WRITE => {
                let guard = Arc::clone(&export.engine).write_owned().await;
                let w = Arc::clone(&wr);
                start_request(&mut writes, &wr, async move {
                    serve_write(guard, req, data, &w).await
                });
            }
            CMD_WRITE_ZEROES | CMD_TRIM => {
                let guard = Arc::clone(&export.engine).write_owned().await;
                let w = Arc::clone(&wr);
                start_request(&mut writes, &wr, async move {
                    serve_zeroes(guard, req, &w).await
                });
            }
            CMD_READ | CMD_BLOCK_STATUS => {
                let guard = Arc::clone(&export.engine).read_owned().await;
                let w = Arc::clone(&wr);
                start_request(&mut reads, &wr, async move {
                    serve_read(guard, conn, req, &w).await
                });
            }
            CMD_FLUSH => {
                if let Err(e) = drain(&mut writes).await {
                    break Err(e);
                }
                let err = if req.offset != 0 || req.len != 0 {
                    EINVAL
                } else if export.read_only || export.engine.read().await.is_clean() {
                    0 // nothing to flush
                } else {
                    export
                        .engine
                        .write()
                        .await
                        .commit(CommitKind::Flush)
                        .await
                        .map(|_| 0)
                        .unwrap_or_else(|e| map_err(&e))
                };
                if let Err(e) = send(&wr, &[&simple_reply(err, req.cookie)]).await {
                    break Err(e);
                }
            }
            CMD_DISC => {
                let drained = match drain(&mut writes).await {
                    Ok(()) => drain(&mut reads).await,
                    e => e,
                };
                // Courtesy commit (design §8.3); clients must still flush.
                if !export.read_only {
                    let _ = export
                        .engine
                        .write()
                        .await
                        .commit(CommitKind::Checkpoint)
                        .await;
                }
                break drained;
            }
            _ => {
                // CACHE, unknown.
                start_request(&mut reads, &wr, {
                    let w = Arc::clone(&wr);
                    async move { send(&w, &[&simple_reply(ENOTSUP, req.cookie)]).await }
                });
            }
        }
    };
    // Reads' replies can no longer be useful; writes finish applying.
    reads.abort_all();
    let mut out = result;
    for done in [drain(&mut writes).await, drain(&mut reads).await] {
        if out.is_ok() {
            out = done;
        }
    }
    out
}

/// The error a request will be refused with, decided before it takes an
/// engine guard: refusals touch no engine state, so they need no place
/// in the receive order. `None` = serve it.
fn refusal(export: &Export, conn: ConnState, req: &Request) -> Option<u32> {
    match req.cmd {
        CMD_WRITE | CMD_WRITE_ZEROES | CMD_TRIM if export.read_only => Some(EPERM),
        CMD_WRITE => validate_io(export, req.offset, req.len, true),
        // No payload on the wire in either direction, so the HARD_MAX_IO
        // cap does not apply — mkfs discards the whole device in a single
        // TRIM (dmesg: EINVAL/err 22).
        CMD_WRITE_ZEROES | CMD_TRIM => validate_io_unbounded(export, req.offset, req.len),
        CMD_READ => validate_io(export, req.offset, req.len, false),
        CMD_BLOCK_STATUS => {
            // No HARD_MAX_IO cap here either: BLOCK_STATUS transfers no
            // data payload, and clients (qemu-img map) legitimately
            // query spans far larger than the max IO size.
            let end = req.offset.checked_add(req.len as u64);
            (!conn.structured
                || !conn.allocation_ctx
                || req.len == 0
                || end.is_none_or(|e| e > export.size))
            .then_some(EINVAL)
        }
        _ => None,
    }
}

/// Commit after a FUA write-class request, under its guard.
async fn fua(engine: &mut DynEngine, result: Result<(), u32>) -> Result<(), u32> {
    match result {
        Ok(()) => engine
            .commit(CommitKind::Flush)
            .await
            .map(|_| ())
            .map_err(|e| map_err(&e)),
        e => e,
    }
}

/// WRITE: applied under its receive-order guard (and committed first
/// when FUA is set); the reply goes out after the guard is released.
/// Already validated ([`refusal`]).
async fn serve_write<St: NbdStream>(
    mut engine: tokio::sync::OwnedRwLockWriteGuard<DynEngine>,
    req: Request,
    data: Vec<u8>,
    wr: &Replies<St>,
) -> std::io::Result<()> {
    let mut result = engine.write(req.offset, &data).map_err(|e| map_err(&e));
    if req.flags & CMD_FLAG_FUA != 0 {
        result = fua(&mut engine, result).await;
    }
    drop(engine);
    send(wr, &[&simple_reply(result.err().unwrap_or(0), req.cookie)]).await
}

/// WRITE_ZEROES / TRIM, like [`serve_write`].
async fn serve_zeroes<St: NbdStream>(
    mut engine: tokio::sync::OwnedRwLockWriteGuard<DynEngine>,
    req: Request,
    wr: &Replies<St>,
) -> std::io::Result<()> {
    let mut result = engine
        .write_zeroes(req.offset, req.len as u64)
        .map_err(|e| map_err(&e));
    if req.cmd == CMD_WRITE_ZEROES && req.flags & CMD_FLAG_FUA != 0 {
        result = fua(&mut engine, result).await;
    }
    drop(engine);
    send(wr, &[&simple_reply(result.err().unwrap_or(0), req.cookie)]).await
}

/// READ and BLOCK_STATUS (already validated by [`refusal`]), under a
/// shared receive-order guard held for the
/// engine call only: every reply, error replies included, is written
/// after it is released.
async fn serve_read<St: NbdStream>(
    guard: tokio::sync::OwnedRwLockReadGuard<DynEngine>,
    conn: ConnState,
    req: Request,
    wr: &Replies<St>,
) -> std::io::Result<()> {
    if req.cmd == CMD_READ {
        // Alignment shim: engine requires 512-aligned IO; NBD clients
        // may not send it (design §8.1 advertises 512 min, but tolerate
        // probing reads).
        let a_off = req.offset & !511;
        let a_end = (req.offset + req.len as u64).div_ceil(512) * 512;
        let result = guard.read(a_off, a_end - a_off).await;
        drop(guard);
        let buf = match result {
            Ok(buf) => buf,
            Err(e) => return send_err(wr, conn, req.cookie, map_err(&e)).await,
        };
        let s = (req.offset - a_off) as usize;
        let data = &buf[s..s + req.len as usize];
        return if conn.structured {
            // Single OFFSET_DATA chunk (design §8.3: DF trivially honored).
            let hdr = structured_reply(
                REPLY_FLAG_DONE,
                REPLY_TYPE_OFFSET_DATA,
                req.cookie,
                8 + req.len,
            );
            send(wr, &[&hdr, &req.offset.to_be_bytes(), data]).await
        } else {
            send(wr, &[&simple_reply(0, req.cookie), data]).await
        };
    }

    // BLOCK_STATUS (validated, like READ, by [`refusal`]).
    let a_off = req.offset & !511;
    let a_end = (req.offset + req.len as u64).div_ceil(512) * 512;
    let result = guard.block_status(a_off, a_end - a_off).await;
    drop(guard);
    let runs = match result {
        Ok(r) => r,
        Err(e) => return send_err(wr, conn, req.cookie, map_err(&e)).await,
    };
    // Trim the aligned cover back to the requested window.
    let mut descs: Vec<(u32, u32)> = Vec::new();
    let mut pos = a_off;
    let req_end = req.offset + req.len as u64;
    for (rlen, alloc) in runs {
        let start = pos.max(req.offset);
        let end = (pos + rlen).min(req_end);
        pos += rlen;
        if end <= start {
            continue;
        }
        let status = if alloc { 0 } else { STATE_HOLE | STATE_ZERO };
        match descs.last_mut() {
            Some(last) if last.1 == status => last.0 += (end - start) as u32,
            _ => descs.push(((end - start) as u32, status)),
        }
    }
    if req.flags & CMD_FLAG_REQ_ONE != 0 {
        descs.truncate(1);
    }
    let payload_len = 4 + 8 * descs.len() as u32;
    let mut frame = Vec::with_capacity(20 + payload_len as usize);
    frame.extend_from_slice(&structured_reply(
        REPLY_FLAG_DONE,
        REPLY_TYPE_BLOCK_STATUS,
        req.cookie,
        payload_len,
    ));
    frame.extend_from_slice(&ALLOCATION_CTX_ID.to_be_bytes());
    for (l, st) in descs {
        frame.extend_from_slice(&l.to_be_bytes());
        frame.extend_from_slice(&st.to_be_bytes());
    }
    send(wr, &[&frame]).await
}

/// Bounds/format validation shared by all IO commands. Returns an NBD
/// error code, or None if acceptable. Writes additionally require 512
/// alignment (we advertise it; reads get the shim instead).
fn validate_io_unbounded(export: &Export, offset: u64, len: u32) -> Option<u32> {
    if len == 0 {
        return Some(EINVAL);
    }
    let Some(end) = offset.checked_add(len as u64) else {
        return Some(EINVAL);
    };
    if end > export.size {
        return Some(EINVAL);
    }
    if !offset.is_multiple_of(512) || !(len as u64).is_multiple_of(512) {
        return Some(EINVAL);
    }
    None
}

fn validate_io(export: &Export, offset: u64, len: u32, aligned: bool) -> Option<u32> {
    if len == 0 || len > HARD_MAX_IO {
        return Some(EINVAL);
    }
    let Some(end) = offset.checked_add(len as u64) else {
        return Some(EINVAL);
    };
    if end > export.size {
        return Some(EINVAL);
    }
    if aligned && (!offset.is_multiple_of(512) || !(len as u64).is_multiple_of(512)) {
        return Some(EINVAL);
    }
    None
}

fn map_err(e: &EngineError) -> u32 {
    match e {
        EngineError::InvalidIo(_) => EINVAL,
        // Both shapes of the I2 self-fence: the head (or the volume's
        // ownership) moved under us — drop the client, never ack.
        EngineError::Conflict | EngineError::Fenced { .. } => ESHUTDOWN,
        _ => EIO,
    }
}

#[cfg(test)]
mod transmission_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    /// A failed task must not stop the join: the set is dropped next,
    /// which would abort a write still applying or committing.
    #[tokio::test]
    async fn drain_joins_every_task_after_a_failure() {
        let mut tasks = Tasks::new();
        let finished = Arc::new(AtomicBool::new(false));
        tasks.spawn(async { Err(std::io::Error::other("first")) });
        let f = Arc::clone(&finished);
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            f.store(true, Ordering::SeqCst);
            Ok(())
        });
        let err = drain(&mut tasks).await.unwrap_err();
        assert_eq!(err.to_string(), "first");
        assert!(
            finished.load(Ordering::SeqCst),
            "drain stopped at the failure"
        );
    }

    /// A request that never waits is answered inline, with no task (a
    /// spawn per request made QD1 slower and hot QD32 about half as
    /// fast); its failure still reaches `settle`. One that waits gets a
    /// task, which finishes once its wait ends. That the inline poll's
    /// no-op waker can't strand it rests on tokio polling every new task
    /// at once, and on each primitive re-registering its waker when
    /// polled (see `start`); a test here couldn't fail on it.
    #[tokio::test]
    async fn only_a_request_that_waits_gets_a_task() {
        let mut tasks = Tasks::new();
        let ran = Arc::new(AtomicBool::new(false));
        let r = Arc::clone(&ran);
        start(&mut tasks, async move {
            r.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(ran.load(Ordering::SeqCst), "not run inline");
        assert!(tasks.is_empty(), "a request that never waited was spawned");

        start(&mut tasks, async { Err(std::io::Error::other("refused")) });
        let (mut none, mut empty) = (Tasks::new(), Tasks::new());
        assert_eq!(
            settle(&mut tasks, &mut none, 0)
                .await
                .unwrap_err()
                .to_string(),
            "refused"
        );

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        start(&mut tasks, async move {
            rx.await.map_err(std::io::Error::other)
        });
        assert_eq!(tasks.len(), 1, "a waiting request has no task");
        tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), settle(&mut tasks, &mut empty, 0))
            .await
            .expect("the spawned request never finished")
            .unwrap();
    }

    /// A reply cancelled part-way through its frame poisons the stream:
    /// the next reply fails rather than landing inside the truncated
    /// payload, where the client would read it as data.
    #[tokio::test]
    async fn a_reply_cut_off_mid_frame_poisons_the_stream() {
        let (ours, mut peer) = tokio::io::duplex(64);
        let (rd, w) = tokio::io::split(ours);
        let wr: Replies<tokio::io::DuplexStream> =
            Arc::new(tokio::sync::Mutex::new(Wire { w, poisoned: false }));
        let big = Arc::clone(&wr);
        let task = tokio::spawn(async move { send(&big, &[&[7u8; 4096]]).await });
        // The 64-byte pipe fills and the send blocks mid-frame.
        tokio::time::sleep(Duration::from_millis(20)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());

        // Fails at once; unpoisoned, it would block on the full pipe (or,
        // with a reader, append its frame to the truncated one).
        let err = tokio::time::timeout(Duration::from_secs(5), send(&wr, &[&[1u8; 16]]))
            .await
            .expect("the reply after a cut-off frame was attempted")
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        drop((wr, rd));
        let mut got = Vec::new();
        peer.read_to_end(&mut got).await.unwrap();
        assert_eq!(
            got,
            vec![7u8; 64],
            "a later reply followed the cut-off frame"
        );
    }
}
