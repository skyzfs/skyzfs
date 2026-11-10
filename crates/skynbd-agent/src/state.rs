//! Durable per-volume agent state (V3.2 §15.1; agent design doc §4).
//!
//! One `SAGT`-framed postcard record per managed volume. Writes are atomic
//! and durable (`tmp` → fsync → rename → dir fsync). A torn or corrupt file
//! is **not fatal**: it loads as [`LoadedState::Corrupt`] and the planner
//! reconstructs from observation (agent design doc §4.3).

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use skynbd_format::Codec;

/// Cycle phase (agent design doc §4.2). Transitions are the only writers of
/// the state file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    Capturing,
    Captured,
    Prepared,
    Backfilling,
    Publishable,
    Visible,
    Promoting,
    /// The §3 advance tail of an advance cycle (D-M15): published and
    /// pinned; repoint → fence → reclaim → rebaselined pair remain.
    Advancing,
    Aborting,
}

/// A local thin snapshot's identity: enough to find it again and to detect
/// pool-generation drift (V3.2 §15.1 "thin device id and pool transaction id").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSnap {
    pub lv_name: String,
    pub thin_dev_id: u64,
    pub pool_tx_id: u64,
    pub created_at_ms: u64,
}

/// The baseline pair `B_k == G_k` (V3.2 §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselinePair {
    pub local: LocalSnap,
    /// Remote visible snapshot id (`G_k` ref name, e.g. `snap/g3`).
    pub remote: String,
    /// Pin holder protecting `G_k` (V3.2 §15.4 pin transfer).
    pub pin_holder: String,
}

/// In-flight prepared target (`P_n` → `G_n`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRef {
    pub snap_id: String,
    /// Target visible ref name for publish (`G_n`).
    pub target: String,
}

/// Durable op cursor: the next `op_id` to assign; ops `< next_op_id` may or
/// may not be durable server-side — the server watermark is authoritative
/// (server design §9A.6), this is only a hint to skip recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpCursor {
    pub next_op_id: u64,
}

/// A CSI snapshot request whose captured candidate has been published as
/// `target`, still owed its alias + request completion (D-M12). Durable so
/// a retry after a crash or an API restart aliases exactly the cut the
/// caller was told about, never a later capture under a reused name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedRequest {
    pub name: String,
    /// Published generation ref (`snap/gN`) holding this request's cut.
    pub target: String,
    /// Unix ms of the local capture this request is bound to.
    pub capture_time_ms: u64,
}

/// The §15.1 record. Field-for-field per the agent design doc §4.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentVolumeState {
    pub volume: String,
    pub phase: Phase,
    pub baseline: Option<BaselinePair>,
    pub candidate: Option<LocalSnap>,
    pub prepared: Option<PreparedRef>,
    pub cursor: Option<OpCursor>,
    /// This cycle is an advance cycle (D-M15 §4): its promote runs the
    /// §3 sequence (repoint, fence, reclaim) and ends rebaselined.
    /// Durable from capture on, so a crash resumes the right tail.
    pub advancing: bool,
    /// The stage-time lease epoch (Invariant F, task 15h): the fence
    /// token every agent transaction for this volume carries. `None` =
    /// staged before 15h or without leases (unfenced legacy behavior).
    /// (No serde default: postcard is not self-describing, so a default
    /// could never apply. `unframe`'s version gate only rejects records
    /// NEWER than `VERSION`; a pre-v3 record passes the gate and then
    /// fails in postcard, which hits EOF reading the appended fields —
    /// either way the record decodes Corrupt and the planner
    /// reconstructs from observation.)
    pub epoch: Option<u64>,
    /// Advances completed since the last two-pool swing — the §7 "aged"
    /// input (`swingEveryNAdvances`). Reset to 0 by a completed swing.
    pub advances_since_swing: u32,
    /// Slot state of an in-flight swing (§7 crash windows): `None` =
    /// steady (no swing in flight), `Some(false)` = swing armed, slot B
    /// is garbage on recovery, `Some(true)` = the flip is durable, slot
    /// A is garbage on recovery. The strategy choice itself is never
    /// durable — only this slot state is.
    pub swing_flipped: Option<bool>,
    /// D-M12 snapshot requests bound to `candidate`: exactly the names
    /// pending when that candidate's freeze began. Bound atomically with
    /// the candidate, so the cut a caller is told about (its
    /// `created_at_ms`) is the one these names will alias.
    pub candidate_requests: Vec<String>,
    /// Requests whose candidate was published and that still need their
    /// alias + completion. Drained by the cycle loop.
    pub served_requests: Vec<ServedRequest>,
    /// Requests whose candidate was discarded unpublished. Their cut is
    /// gone, so they must never be re-captured under the same name: held
    /// here until the control plane acknowledges them as lost.
    pub lost_requests: Vec<String>,
    pub updated_at_ms: u64,
}

impl Codec for AgentVolumeState {
    const MAGIC: [u8; 4] = *b"SAGT";
    // v5 added the D-M12 request bindings (`candidate_requests`,
    // `served_requests`, `lost_requests`); v4 added
    // `advances_since_swing` + `swing_flipped` (hybrid
    // reclaim, task 15i); v3 added `epoch` (Invariant F, task 15h); v2
    // added `advancing` (D-M15 task 15c). Old records decode as Corrupt
    // and the planner reconstructs from observation — by design, and
    // the no-legacy posture (#76's rule) makes the one-time re-seed on
    // upgrade the accepted cost: an older record is dropped, the planner
    // rebuilds the pair from the published generation, and the first
    // cycle re-derives the mask state from the kernel anyway.
    const VERSION: u16 = 5;
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode: {0}")]
    Format(#[from] skynbd_format::FormatError),
}

/// Result of loading a volume's state file.
#[derive(Debug)]
pub enum LoadedState {
    /// No state file — first run on this host, or state dir loss.
    Absent,
    /// A file exists but does not decode (torn write, corruption). The
    /// planner treats this as "no trustworthy local record".
    Corrupt,
    Present(Box<AgentVolumeState>),
}

/// Atomic fsync'd store: `{dir}/{volume}.state`.
pub struct StateStore {
    dir: PathBuf,
}

impl StateStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, volume: &str) -> PathBuf {
        self.dir.join(format!("{volume}.state"))
    }

    /// Persist atomically: write `.tmp`, fsync file, rename over the final
    /// name, fsync the directory (agent design doc §4.3).
    pub fn save(&self, state: &AgentVolumeState) -> Result<(), StateError> {
        fs::create_dir_all(&self.dir)?;
        let bytes = state.encode()?;
        let tmp = self.dir.join(format!("{}.state.tmp", state.volume));
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, self.path(&state.volume))?;
        // Durable rename: fsync the containing directory.
        File::open(&self.dir)?.sync_all()?;
        Ok(())
    }

    pub fn load(&self, volume: &str) -> Result<LoadedState, StateError> {
        let bytes = match fs::read(self.path(volume)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LoadedState::Absent),
            Err(e) => return Err(e.into()),
        };
        Ok(match AgentVolumeState::decode(&bytes) {
            Ok(s) => LoadedState::Present(Box::new(s)),
            Err(_) => LoadedState::Corrupt,
        })
    }

    /// True if a stale `.tmp` from a crashed save exists (harmless; cleaned
    /// on the next save, surfaced for tests and `state-dump`).
    pub fn stale_tmp(&self, volume: &str) -> bool {
        self.dir.join(format!("{volume}.state.tmp")).exists()
    }
}

pub fn state_path_for(dir: &Path, volume: &str) -> PathBuf {
    dir.join(format!("{volume}.state"))
}
