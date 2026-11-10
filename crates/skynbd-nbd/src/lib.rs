//! NBD protocol server (design §8).
//!
//! M2 scope: fixed-newstyle negotiation (`LIST`, `GO`, `INFO`,
//! `EXPORT_NAME`, `ABORT`), simple-reply transmission with `READ`/`WRITE`/
//! `DISC`/`FLUSH`/`TRIM`/`WRITE_ZEROES` + FUA, exact error mapping
//! (§8.3), and the export registry. Interop pinned by golden-byte tests
//! plus gated `qemu-io`/`nbdinfo` tests (scenarios S1–S2).
//!
//! Deferred to M4 (design §8.3): structured replies, `BLOCK_STATUS`
//! (`base:allocation`), `NBD_OPT_SET_META_CONTEXT`. Deferred to M3+:
//! registry derivation from ref-store watch events.
//!
//! STARTTLS (auth design §3): config-gated forced-TLS mode with mTLS
//! client verification and per-identity export ACLs — `tls` and `acl`
//! modules, wired by the daemon.

pub mod acl;
pub mod origin;
pub mod proto;
mod server;
mod sync;
pub mod tls;

pub use acl::{AclRule, ConnAccess};
pub use server::{DynEngine, Export, ExportRegistry, Server, MAX_IN_FLIGHT_LIMIT};
pub use sync::{spawn_registry_sync, VolumeBinding};
pub use tls::TlsContext;
