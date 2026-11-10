//! STARTTLS + mTLS + ACL tests (auth design §3): forced-TLS spec
//! compliance, client-cert identity → export authorization, handshake
//! rejection, acceptor hot-swap, and the negotiation timeout — all with
//! an in-crate rustls NBD client against a live TCP server.

use std::sync::Arc;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, Ia5String, IsCa, KeyPair, SanType,
};
use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::proto::*;
use skynbd_nbd::{AclRule, DynEngine, Export, ExportRegistry, Server, TlsContext};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

const VOL_BYTES: u64 = 256 * 1024;

// ------------------------------------------------------------- PKI rig

struct TestCa {
    cert: Certificate,
    key: KeyPair,
}

impl TestCa {
    fn new() -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "skynbd-test-ca");
        let cert = params.self_signed(&key).unwrap();
        Self { cert, key }
    }

    /// Server cert with a `localhost` DNS SAN.
    fn server_cert(&self) -> (String, String) {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = params.signed_by(&key, &self.cert, &self.key).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    /// Client cert whose identity is a SPIFFE-style URI SAN.
    fn client_cert(&self, spiffe: &str) -> (String, String) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .subject_alt_names
            .push(SanType::URI(Ia5String::try_from(spiffe).unwrap()));
        let cert = params.signed_by(&key, &self.cert, &self.key).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    fn ca_pem(&self) -> String {
        self.cert.pem()
    }
}

fn server_tls_config(ca: &TestCa) -> Arc<rustls::ServerConfig> {
    let (cert, key) = ca.server_cert();
    skynbd_nbd::tls::build_server_config(cert.as_bytes(), key.as_bytes(), ca.ca_pem().as_bytes())
        .unwrap()
}

/// rustls client config trusting `server_ca`, presenting `client` cert
/// (None = no client auth offered).
fn client_tls_config(
    server_ca: &TestCa,
    client: Option<(&str, &str)>,
) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut server_ca.ca_pem().as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots);
    let config = match client {
        None => builder.with_no_client_auth(),
        Some((cert_pem, key_pem)) => {
            let certs = rustls_pemfile::certs(&mut cert_pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
                .unwrap()
                .unwrap();
            builder.with_client_auth_cert(certs, key).unwrap()
        }
    };
    Arc::new(config)
}

// ---------------------------------------------------------- server rig

async fn start_server(
    tls: Arc<TlsContext>,
    exports: &[&str],
    negotiation_timeout: std::time::Duration,
) -> std::net::SocketAddr {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let registry = Arc::new(ExportRegistry::new());
    for name in exports {
        let engine: DynEngine = Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            VolumeId(ulid::Ulid::new()),
            VOL_BYTES,
            EngineConfig::default(),
        )
        .await
        .unwrap();
        registry
            .register(
                name,
                Export {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                    read_only: false,
                    size: VOL_BYTES,
                },
            )
            .await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        // Concurrent servicing on (issue #97 item 3): every TLS scenario
        // also runs the split, shared-write-half reply path.
        Server::new(registry)
            .with_tls(tls)
            .negotiation_timeout(negotiation_timeout)
            .max_in_flight(16)
            .run(listener),
    );
    addr
}

fn agent_acl() -> Vec<AclRule> {
    vec![
        AclRule {
            identity: "spiffe://test/agent/*".into(),
            exports: vec!["vol/head".into()],
            write: true,
        },
        AclRule {
            identity: "spiffe://test/restore/*".into(),
            exports: vec!["vol/head".into()],
            write: false,
        },
    ]
}

// ---------------------------------------------------------- mini client

async fn read_greeting<S: AsyncRead + Unpin>(s: &mut S) {
    let mut greeting = [0u8; 18];
    s.read_exact(&mut greeting).await.unwrap();
    assert_eq!(&greeting[0..8], &NBDMAGIC.to_be_bytes());
}

async fn send_opt<S: AsyncWrite + Unpin>(s: &mut S, opt: u32, payload: &[u8]) {
    s.write_all(&IHAVEOPT.to_be_bytes()).await.unwrap();
    s.write_all(&opt.to_be_bytes()).await.unwrap();
    s.write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    s.write_all(payload).await.unwrap();
}

/// (reply type, payload)
async fn read_opt_reply<S: AsyncRead + Unpin>(s: &mut S) -> (u32, Vec<u8>) {
    let magic = s.read_u64().await.unwrap();
    assert_eq!(magic, OPT_REPLY_MAGIC);
    let _opt = s.read_u32().await.unwrap();
    let rep = s.read_u32().await.unwrap();
    let len = s.read_u32().await.unwrap();
    let mut payload = vec![0u8; len as usize];
    s.read_exact(&mut payload).await.unwrap();
    (rep, payload)
}

fn go_payload(name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&(name.len() as u32).to_be_bytes());
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    p
}

/// OPT_GO: Ok(tflags) on success, Err(reply type) on option error.
async fn go<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, name: &str) -> Result<u16, u32> {
    send_opt(s, OPT_GO, &go_payload(name)).await;
    let mut tflags = 0u16;
    loop {
        let (rep, payload) = read_opt_reply(s).await;
        match rep {
            REP_INFO => {
                if u16::from_be_bytes(payload[0..2].try_into().unwrap()) == INFO_EXPORT {
                    tflags = u16::from_be_bytes(payload[10..12].try_into().unwrap());
                }
            }
            REP_ACK => return Ok(tflags),
            other => return Err(other),
        }
    }
}

/// Handshake through STARTTLS: returns the TLS stream ready for options.
async fn tls_client(
    addr: std::net::SocketAddr,
    config: Arc<rustls::ClientConfig>,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let mut tcp = TcpStream::connect(addr).await?;
    read_greeting(&mut tcp).await;
    tcp.write_all(&(CFLAG_FIXED_NEWSTYLE | CFLAG_NO_ZEROES).to_be_bytes())
        .await?;
    send_opt(&mut tcp, OPT_STARTTLS, &[]).await;
    let (rep, _) = read_opt_reply(&mut tcp).await;
    assert_eq!(rep, REP_ACK);
    let connector = tokio_rustls::TlsConnector::from(config);
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    connector.connect(name, tcp).await
}

async fn write_block<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, cookie: u64) -> u32 {
    let mut req = [0u8; 28];
    req[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
    req[6..8].copy_from_slice(&CMD_WRITE.to_be_bytes());
    req[8..16].copy_from_slice(&cookie.to_be_bytes());
    req[16..24].copy_from_slice(&0u64.to_be_bytes());
    req[24..28].copy_from_slice(&512u32.to_be_bytes());
    s.write_all(&req).await.unwrap();
    s.write_all(&[0xAB; 512]).await.unwrap();
    let magic = s.read_u32().await.unwrap();
    assert_eq!(magic, SIMPLE_REPLY_MAGIC);
    let err = s.read_u32().await.unwrap();
    let _cookie = s.read_u64().await.unwrap();
    err
}

// -------------------------------------------------------------- tests

/// FORCEDTLS: pre-TLS options are answered REP_ERR_TLS_REQD; export
/// names never cross in cleartext.
#[tokio::test(flavor = "multi_thread")]
async fn pre_tls_options_get_tls_reqd() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_secs(30)).await;

    let mut tcp = TcpStream::connect(addr).await.unwrap();
    read_greeting(&mut tcp).await;
    tcp.write_all(&CFLAG_FIXED_NEWSTYLE.to_be_bytes())
        .await
        .unwrap();
    send_opt(&mut tcp, OPT_LIST, &[]).await;
    let (rep, _) = read_opt_reply(&mut tcp).await;
    assert_eq!(rep, REP_ERR_TLS_REQD);
    send_opt(&mut tcp, OPT_GO, &go_payload("vol/head")).await;
    let (rep, _) = read_opt_reply(&mut tcp).await;
    assert_eq!(rep, REP_ERR_TLS_REQD);
    // ABORT still works pre-TLS.
    send_opt(&mut tcp, OPT_ABORT, &[]).await;
    let (rep, _) = read_opt_reply(&mut tcp).await;
    assert_eq!(rep, REP_ACK);
}

/// Write-granted identity attaches and writes; LIST shows only its
/// granted exports.
#[tokio::test(flavor = "multi_thread")]
async fn mtls_identity_attaches_within_acl() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(
        tls,
        &["vol/head", "secret/head"],
        std::time::Duration::from_secs(30),
    )
    .await;

    let (cert, key) = ca.client_cert("spiffe://test/agent/a1");
    let config = client_tls_config(&ca, Some((&cert, &key)));
    let mut s = tls_client(addr, config).await.unwrap();

    // LIST filtered: secret/head is invisible.
    send_opt(&mut s, OPT_LIST, &[]).await;
    let mut names = Vec::new();
    loop {
        let (rep, payload) = read_opt_reply(&mut s).await;
        match rep {
            REP_SERVER => names.push(String::from_utf8(payload[4..].to_vec()).unwrap()),
            REP_ACK => break,
            other => panic!("unexpected reply {other}"),
        }
    }
    assert_eq!(names, vec!["vol/head"]);

    let tflags = go(&mut s, "vol/head").await.unwrap();
    assert_eq!(tflags & TFLAG_READ_ONLY, 0);
    assert_eq!(write_block(&mut s, 1).await, 0);
}

/// Pipelined requests over TLS: 16 writes of 16 KiB (the whole volume)
/// then 16 reads of the same blocks, all sent without waiting, while
/// replies are read concurrently. Every reply arrives whole (each is
/// flushed through the TLS session) and every byte is right.
#[tokio::test(flavor = "multi_thread")]
async fn pipelined_requests_over_tls() {
    const BLK: usize = 16 * 1024;
    const N: u64 = 16;
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_secs(30)).await;
    let (cert, key) = ca.client_cert("spiffe://test/agent/a1");
    let mut s = tls_client(addr, client_tls_config(&ca, Some((&cert, &key))))
        .await
        .unwrap();
    go(&mut s, "vol/head").await.unwrap();
    let block = |i: u64| vec![i as u8 ^ 0x3C; BLK];
    let (mut rd, mut wr) = tokio::io::split(s);
    let send = async move {
        let req = |cmd: u16, cookie: u64, off: u64| {
            let mut h = [0u8; 28];
            h[0..4].copy_from_slice(&REQUEST_MAGIC.to_be_bytes());
            h[6..8].copy_from_slice(&cmd.to_be_bytes());
            h[8..16].copy_from_slice(&cookie.to_be_bytes());
            h[16..24].copy_from_slice(&off.to_be_bytes());
            h[24..28].copy_from_slice(&(BLK as u32).to_be_bytes());
            h
        };
        for i in 0..N {
            wr.write_all(&req(CMD_WRITE, i, i * BLK as u64))
                .await
                .unwrap();
            wr.write_all(&block(i)).await.unwrap();
        }
        for i in 0..N {
            wr.write_all(&req(CMD_READ, 100 + i, i * BLK as u64))
                .await
                .unwrap();
        }
        wr.flush().await.unwrap();
        wr
    };
    let recv = async move {
        let mut writes_acked = 0;
        let mut reads = std::collections::BTreeMap::new();
        for _ in 0..2 * N {
            assert_eq!(rd.read_u32().await.unwrap(), SIMPLE_REPLY_MAGIC);
            assert_eq!(rd.read_u32().await.unwrap(), 0);
            let cookie = rd.read_u64().await.unwrap();
            if cookie < 100 {
                writes_acked += 1;
            } else {
                let mut data = vec![0u8; BLK];
                rd.read_exact(&mut data).await.unwrap();
                reads.insert(cookie - 100, data);
            }
        }
        (writes_acked, reads)
    };
    let (_wr, (writes_acked, reads)) = tokio::join!(send, recv);
    assert_eq!(writes_acked, N);
    assert_eq!(reads.len(), N as usize);
    for (i, data) in reads {
        assert!(data == block(i), "block {i} read back wrong");
    }
}

/// A read-only grant forces the export read-only for the connection:
/// READ_ONLY transmission flag, writes get EPERM.
#[tokio::test(flavor = "multi_thread")]
async fn read_only_grant_forces_ro() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_secs(30)).await;

    let (cert, key) = ca.client_cert("spiffe://test/restore/r1");
    let mut s = tls_client(addr, client_tls_config(&ca, Some((&cert, &key))))
        .await
        .unwrap();
    let tflags = go(&mut s, "vol/head").await.unwrap();
    assert_ne!(tflags & TFLAG_READ_ONLY, 0);
    assert_eq!(write_block(&mut s, 2).await, EPERM);
}

/// Denied export is indistinguishable from a nonexistent one.
#[tokio::test(flavor = "multi_thread")]
async fn denied_export_is_unknown() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(
        tls,
        &["vol/head", "secret/head"],
        std::time::Duration::from_secs(30),
    )
    .await;

    let (cert, key) = ca.client_cert("spiffe://test/restore/r1");
    let mut s = tls_client(addr, client_tls_config(&ca, Some((&cert, &key))))
        .await
        .unwrap();
    assert_eq!(go(&mut s, "secret/head").await, Err(REP_ERR_UNKNOWN));
    assert_eq!(go(&mut s, "no/such").await, Err(REP_ERR_UNKNOWN));
}

/// Unbound identity: every export is "unknown" (empty grant set).
#[tokio::test(flavor = "multi_thread")]
async fn unmatched_identity_sees_nothing() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_secs(30)).await;

    let (cert, key) = ca.client_cert("spiffe://test/stranger/x");
    let mut s = tls_client(addr, client_tls_config(&ca, Some((&cert, &key))))
        .await
        .unwrap();
    assert_eq!(go(&mut s, "vol/head").await, Err(REP_ERR_UNKNOWN));
}

/// Client certs are mandatory: no cert or wrong-CA cert never reaches
/// the option phase.
#[tokio::test(flavor = "multi_thread")]
async fn missing_or_untrusted_client_cert_is_rejected() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_secs(30)).await;

    // The rejection may surface at handshake or on first use (TLS 1.3
    // sends the alert after the client Finished) — either way, GO must
    // never succeed.
    for config in [client_tls_config(&ca, None), {
        let other_ca = TestCa::new();
        let (cert, key) = other_ca.client_cert("spiffe://test/agent/a1");
        client_tls_config(&ca, Some((&cert, &key)))
    }] {
        // Raw (non-unwrapping) IO: the server may close at any point
        // between our handshake completing and the read — a write EPIPE
        // is as valid a rejection as a handshake error.
        let outcome = async {
            let mut s = tls_client(addr, config).await?;
            let mut opt = Vec::new();
            opt.extend_from_slice(&IHAVEOPT.to_be_bytes());
            opt.extend_from_slice(&OPT_GO.to_be_bytes());
            let payload = go_payload("vol/head");
            opt.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            opt.extend_from_slice(&payload);
            s.write_all(&opt).await?;
            let magic = s.read_u64().await?;
            assert_eq!(magic, OPT_REPLY_MAGIC, "server replied despite bad cert");
            Ok::<_, std::io::Error>(())
        }
        .await;
        assert!(outcome.is_err(), "expected TLS rejection");
    }
}

/// Hot swap (cert rotation): new handshakes use swapped material.
#[tokio::test(flavor = "multi_thread")]
async fn acceptor_swap_serves_new_material() {
    let ca1 = TestCa::new();
    let ca2 = TestCa::new();
    let ctx = Arc::new(TlsContext::new(server_tls_config(&ca1), agent_acl()));
    let addr = start_server(
        Arc::clone(&ctx),
        &["vol/head"],
        std::time::Duration::from_secs(30),
    )
    .await;

    // Client trusting only CA2 fails against the CA1 server cert...
    let (cert2, key2) = ca2.client_cert("spiffe://test/agent/a1");
    assert!(
        tls_client(addr, client_tls_config(&ca2, Some((&cert2, &key2))))
            .await
            .is_err()
    );
    // ...swap in CA2 material (server cert + client CA), and it works.
    ctx.swap(server_tls_config(&ca2)).await;
    let mut s = tls_client(addr, client_tls_config(&ca2, Some((&cert2, &key2))))
        .await
        .unwrap();
    assert!(go(&mut s, "vol/head").await.is_ok());
}

/// Half-open clients are dropped at the negotiation deadline.
#[tokio::test(flavor = "multi_thread")]
async fn negotiation_timeout_drops_stalled_clients() {
    let ca = TestCa::new();
    let tls = Arc::new(TlsContext::new(server_tls_config(&ca), agent_acl()));
    let addr = start_server(tls, &["vol/head"], std::time::Duration::from_millis(300)).await;

    let mut tcp = TcpStream::connect(addr).await.unwrap();
    read_greeting(&mut tcp).await;
    // Send nothing further; server must hang up.
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), tcp.read(&mut buf))
        .await
        .expect("server did not drop the stalled connection");
    assert!(matches!(read, Ok(0) | Err(_)), "expected EOF/reset");
}
