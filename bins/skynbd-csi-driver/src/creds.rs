//! NBD mTLS credentials for the node plugin (auth design §3.2, §9.3):
//! generate a keypair locally, enroll its CSR at the control plane, and
//! keep the resulting cert fresh. The private key never leaves this
//! process's filesystem; nbd-client consumes the PEM files directly.

use std::path::{Path, PathBuf};

use crate::cp;

#[derive(Debug, thiserror::Error)]
pub enum CredsError {
    #[error("keygen/csr: {0}")]
    Csr(String),
    #[error("enroll: {0}")]
    Enroll(#[from] cp::CpError),
    #[error("write {0}: {1}")]
    Io(PathBuf, std::io::Error),
}

/// Enroll (or re-enroll) into `dir`, atomically replacing cert/key/ca so
/// an in-flight nbd-client never reads a half-written pair. File names
/// (client-cert.pem / client-key.pem / ca-cert.pem) are the contract
/// [`crate::node::NodeMechanics`] builds nbd-client flags from. Returns
/// the issued identity (the URI SAN NBD ACLs match).
pub async fn enroll_into(client: &cp::Client, dir: &Path) -> Result<String, CredsError> {
    let key = rcgen::KeyPair::generate().map_err(|e| CredsError::Csr(e.to_string()))?;
    let params = rcgen::CertificateParams::new(Vec::<String>::new())
        .map_err(|e| CredsError::Csr(e.to_string()))?;
    let csr = params
        .serialize_request(&key)
        .map_err(|e| CredsError::Csr(e.to_string()))?
        .pem()
        .map_err(|e| CredsError::Csr(e.to_string()))?;

    let issued = client.enroll(&csr).await?;

    std::fs::create_dir_all(dir).map_err(|e| CredsError::Io(dir.to_path_buf(), e))?;
    let write = |name: &str, data: &str, mode: u32| -> Result<PathBuf, CredsError> {
        use std::os::unix::fs::PermissionsExt;
        let tmp = dir.join(format!(".{name}.tmp"));
        let path = dir.join(name);
        std::fs::write(&tmp, data).map_err(|e| CredsError::Io(tmp.clone(), e))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
            .map_err(|e| CredsError::Io(tmp.clone(), e))?;
        std::fs::rename(&tmp, &path).map_err(|e| CredsError::Io(path.clone(), e))?;
        Ok(path)
    };
    write("client-key.pem", &key.serialize_pem(), 0o600)?;
    write("client-cert.pem", &issued.cert_pem, 0o644)?;
    write("ca-cert.pem", &issued.ca_pem, 0o644)?;
    tracing::info!(identity = %issued.identity, not_after = %issued.not_after,
        "enrolled NBD client cert");
    Ok(issued.identity)
}

/// Renewal loop: re-enroll every `interval`, retrying sooner on failure.
/// Certs outlive `interval` by design (ttl_s should be ≥ 2×), so one
/// failed cycle is a warning, not an outage (auth design §9.4: the
/// enrollment path is provision/renew only — in-flight IO never blocks).
pub async fn renew_loop(client: cp::Client, dir: PathBuf, interval: std::time::Duration) {
    let retry = std::time::Duration::from_secs(60).min(interval);
    loop {
        tokio::time::sleep(interval).await;
        let mut wait = retry;
        while let Err(e) = enroll_into(&client, &dir).await {
            tracing::warn!(error = %e, "cert renewal failed; retrying");
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(interval);
        }
    }
}
