//! STARTTLS server context (auth design §3.1).
//!
//! Holds the rustls acceptor (client certs required, verified against a
//! configured CA bundle) plus the export ACL table. The acceptor is
//! hot-swappable: with short-lived certs (§3.2), rotation must not need
//! a daemon restart — new handshakes pick up swapped material while
//! established connections are untouched.

use std::sync::Arc;

use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use tokio::sync::RwLock;
use tokio_rustls::TlsAcceptor;

use crate::acl::AclRule;

pub struct TlsContext {
    acceptor: RwLock<TlsAcceptor>,
    pub acl: Arc<Vec<AclRule>>,
}

impl TlsContext {
    pub fn new(config: Arc<ServerConfig>, acl: Vec<AclRule>) -> Self {
        Self {
            acceptor: RwLock::new(TlsAcceptor::from(config)),
            acl: Arc::new(acl),
        }
    }

    pub async fn acceptor(&self) -> TlsAcceptor {
        self.acceptor.read().await.clone()
    }

    /// Swap in rebuilt cert/key/CA material (cert rotation, §3.2).
    pub async fn swap(&self, config: Arc<ServerConfig>) {
        *self.acceptor.write().await = TlsAcceptor::from(config);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    #[error("bad PEM: {0}")]
    Pem(#[from] std::io::Error),
    #[error("no private key in key PEM")]
    NoKey,
    #[error("no certificates in client CA PEM")]
    EmptyClientCa,
    #[error("client CA: {0}")]
    ClientCa(#[from] rustls::server::VerifierBuilderError),
    #[error("tls config: {0}")]
    Rustls(#[from] rustls::Error),
}

/// Server config requiring client certificates chained to
/// `client_ca_pem` (a bundle: skynbd's own root, an external CA, or
/// both — §3.2). Ring provider selected explicitly: the dependency
/// graph links both rustls providers, so the ambiguous default would
/// panic at runtime.
pub fn build_server_config(
    cert_pem: &[u8],
    key_pem: &[u8],
    client_ca_pem: &[u8],
) -> Result<Arc<ServerConfig>, TlsConfigError> {
    let certs = rustls_pemfile::certs(&mut &cert_pem[..]).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut &key_pem[..])?.ok_or(TlsConfigError::NoKey)?;
    let mut roots = RootCertStore::empty();
    let mut n = 0;
    for cert in rustls_pemfile::certs(&mut &client_ca_pem[..]) {
        roots.add(cert?)?;
        n += 1;
    }
    if n == 0 {
        return Err(TlsConfigError::EmptyClientCa);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier =
        WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()?;
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?;
    Ok(Arc::new(config))
}

/// Identity from a verified client cert: URI SAN first (SPIFFE ID, the
/// recommended scheme — §3.2), then DNS SAN, then subject CN. The ACL
/// table (§3.3) matches on nothing else.
pub fn peer_identity(cert: &CertificateDer<'_>) -> Option<String> {
    use x509_parser::prelude::{GeneralName, ParsedExtension};
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref()).ok()?;
    let mut dns = None;
    for ext in parsed.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for name in &san.general_names {
                match name {
                    GeneralName::URI(u) => return Some(u.to_string()),
                    GeneralName::DNSName(d) if dns.is_none() => {
                        dns = Some(d.to_string());
                    }
                    _ => {}
                }
            }
        }
    }
    dns.or_else(|| {
        parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(str::to_string)
    })
}
