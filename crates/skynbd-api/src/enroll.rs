//! Cert enrollment (auth design §3.2 amendment): `POST /v1/certs/enroll`
//! turns a token-verified identity into a short-lived NBD mTLS client
//! cert signed by skynbd's own root. Issuer and verifier are the same
//! system, so the claim→SAN discipline is enforced by the party that
//! will do the verifying: the caller's CSR contributes *only* its public
//! key — subject, SANs, usages, and validity all come from the server.
//!
//! CA material is re-read from disk on every issuance, so root rotation
//! needs no restart (issuance is rare; two small file reads are noise).

use std::time::Duration;

use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, ExtendedKeyUsagePurpose,
    Ia5String, IsCa, KeyPair, KeyUsagePurpose, SanType,
};

/// Clock-skew allowance: `not_before` is backdated by this much.
const NOT_BEFORE_SKEW: Duration = Duration::from_secs(60);

pub struct CertIssuer {
    pub ca_cert_path: std::path::PathBuf,
    pub ca_key_path: std::path::PathBuf,
    /// SPIFFE trust domain: issued SANs are
    /// `spiffe://{trust_domain}/{path from sub}`.
    pub trust_domain: String,
    pub ttl: Duration,
}

pub struct IssuedCert {
    pub cert_pem: String,
    /// The issuing CA cert (for the client's server-side trust bundle
    /// convenience; NBD server identity is a separate concern).
    pub ca_pem: String,
    /// The URI SAN the cert carries — what NBD ACLs (§3.3) match.
    pub identity: String,
    pub not_after: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    /// Caller-side: unparseable/unverifiable CSR or an identity that
    /// yields no usable SPIFFE path. → 400.
    #[error("bad CSR: {0}")]
    BadCsr(String),
    /// Server-side: CA material unreadable or unusable. → 500.
    #[error("CA unavailable: {0}")]
    Ca(String),
}

impl CertIssuer {
    /// The SPIFFE ID issued for a verified `sub` claim. `:` and `/` act
    /// as segment separators (`system:serviceaccount:prod:agent` →
    /// `spiffe://td/system/serviceaccount/prod/agent`); characters
    /// outside the SPIFFE path alphabet (`A-Za-z0-9._-`) become `-`.
    /// Deterministic, so ACL globs can be written from the sub alone.
    pub fn spiffe_id(&self, sub: &str) -> Option<String> {
        let path: Vec<String> = sub
            .split([':', '/'])
            .filter(|seg| !seg.is_empty())
            .map(|seg| {
                seg.chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect()
            })
            .collect();
        if path.is_empty() {
            return None;
        }
        Some(format!("spiffe://{}/{}", self.trust_domain, path.join("/")))
    }

    /// Parse startup-time sanity: CA cert+key must load and pair up.
    pub fn validate(&self) -> Result<(), EnrollError> {
        self.load_ca().map(|_| ())
    }

    fn load_ca(&self) -> Result<(rcgen::Certificate, KeyPair, String), EnrollError> {
        let ca = |e: &dyn std::fmt::Display| EnrollError::Ca(e.to_string());
        let cert_pem = std::fs::read_to_string(&self.ca_cert_path).map_err(|e| ca(&e))?;
        let key_pem = std::fs::read_to_string(&self.ca_key_path).map_err(|e| ca(&e))?;
        let key = KeyPair::from_pem(&key_pem).map_err(|e| ca(&e))?;
        // Reconstruct an rcgen issuer from the on-disk CA: the signed
        // cert's issuer DN and signature come from these; the on-disk
        // cert itself is what belongs in the NBD trust bundle.
        let params = CertificateParams::from_ca_cert_pem(&cert_pem).map_err(|e| ca(&e))?;
        let cert = params.self_signed(&key).map_err(|e| ca(&e))?;
        Ok((cert, key, cert_pem))
    }

    /// Issue a client cert for `sub` (already verified by API auth). The
    /// CSR is checked for proof-of-possession (self-signature) and then
    /// mined for its public key only.
    pub fn issue(&self, csr_pem: &str, sub: &str) -> Result<IssuedCert, EnrollError> {
        let identity = self
            .spiffe_id(sub)
            .ok_or_else(|| EnrollError::BadCsr("identity yields empty SPIFFE path".into()))?;
        let (ca_cert, ca_key, ca_pem) = self.load_ca()?;

        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)
            .map_err(|e| EnrollError::BadCsr(e.to_string()))?;
        // Nothing identity-bearing survives from the CSR.
        csr.params.distinguished_name = DistinguishedName::new();
        csr.params.subject_alt_names = vec![SanType::URI(
            Ia5String::try_from(identity.as_str())
                .map_err(|e| EnrollError::BadCsr(e.to_string()))?,
        )];
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        csr.params.serial_number = None; // random per issuance
        let now = time::OffsetDateTime::now_utc();
        csr.params.not_before = now - NOT_BEFORE_SKEW;
        csr.params.not_after = now + self.ttl;
        let not_after = csr.params.not_after;

        let cert = csr
            .signed_by(&ca_cert, &ca_key)
            .map_err(|e| EnrollError::Ca(e.to_string()))?;
        Ok(IssuedCert {
            cert_pem: cert.pem(),
            ca_pem,
            identity,
            not_after: not_after
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| not_after.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, data: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// Throwaway CA on disk + an issuer over it.
    fn test_issuer(dir: &std::path::Path) -> CertIssuer {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "skynbd-enroll-test-ca");
        let cert = params.self_signed(&key).unwrap();
        CertIssuer {
            ca_cert_path: write(dir, "ca.pem", &cert.pem()),
            ca_key_path: write(dir, "ca.key", &key.serialize_pem()),
            trust_domain: "td".into(),
            ttl: Duration::from_secs(3600),
        }
    }

    fn make_csr() -> (String, KeyPair) {
        let key = KeyPair::generate().unwrap();
        let params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let csr = params.serialize_request(&key).unwrap();
        (csr.pem().unwrap(), key)
    }

    #[test]
    fn spiffe_id_mapping() {
        let iss = CertIssuer {
            ca_cert_path: "/none".into(),
            ca_key_path: "/none".into(),
            trust_domain: "prod".into(),
            ttl: Duration::from_secs(60),
        };
        assert_eq!(
            iss.spiffe_id("system:serviceaccount:ns:agent").unwrap(),
            "spiffe://prod/system/serviceaccount/ns/agent"
        );
        assert_eq!(iss.spiffe_id("plain").unwrap(), "spiffe://prod/plain");
        // Disallowed chars degrade to '-', separators collapse.
        assert_eq!(iss.spiffe_id("a b::c//d").unwrap(), "spiffe://prod/a-b/c/d");
        assert_eq!(iss.spiffe_id(":::"), None);
        assert_eq!(iss.spiffe_id(""), None);
    }

    #[test]
    fn issue_sets_identity_and_ignores_csr_claims() {
        let dir = tempfile::tempdir().unwrap();
        let issuer = test_issuer(dir.path());

        // A hostile CSR: claims a DN and SANs of its own.
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["evil.example".to_string()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "cluster-admin");
        let csr = params.serialize_request(&key).unwrap().pem().unwrap();

        let issued = issuer
            .issue(&csr, "system:serviceaccount:prod:node-7")
            .unwrap();
        assert_eq!(
            issued.identity,
            "spiffe://td/system/serviceaccount/prod/node-7"
        );

        // The NBD verifier's own extractor must see exactly the issued
        // SPIFFE ID — no CSR-claimed DNS SAN, no CN fallback.
        let der = rustls_pemfile::certs(&mut issued.cert_pem.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(
            skynbd_nbd::tls::peer_identity(&der).as_deref(),
            Some("spiffe://td/system/serviceaccount/prod/node-7")
        );
        let (_, parsed) = x509_parser::parse_x509_certificate(der.as_ref()).unwrap();
        assert!(!parsed
            .subject()
            .iter_common_name()
            .any(|cn| cn.as_str() == Ok("cluster-admin")));
        assert_eq!(parsed.issuer().to_string(), "CN=skynbd-enroll-test-ca");
        let validity = parsed.validity();
        let lifetime = validity.not_after.timestamp() - validity.not_before.timestamp();
        assert_eq!(lifetime as u64, 3600 + NOT_BEFORE_SKEW.as_secs());
    }

    #[test]
    fn garbage_csr_is_bad_request() {
        let dir = tempfile::tempdir().unwrap();
        let issuer = test_issuer(dir.path());
        assert!(matches!(
            issuer.issue("not a csr", "sub"),
            Err(EnrollError::BadCsr(_))
        ));
        let (csr, _) = make_csr();
        assert!(matches!(
            issuer.issue(&csr, ":"),
            Err(EnrollError::BadCsr(_))
        ));
    }

    #[test]
    fn missing_ca_is_server_error_and_validate_catches_it() {
        let issuer = CertIssuer {
            ca_cert_path: "/nonexistent/ca.pem".into(),
            ca_key_path: "/nonexistent/ca.key".into(),
            trust_domain: "td".into(),
            ttl: Duration::from_secs(60),
        };
        assert!(matches!(issuer.validate(), Err(EnrollError::Ca(_))));
        let (csr, _) = make_csr();
        assert!(matches!(issuer.issue(&csr, "sub"), Err(EnrollError::Ca(_))));
    }
}
