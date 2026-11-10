//! Production V3.2 snapshot agent — M8 core (agent design doc §3–§5).
//!
//! This crate deliberately starts with the two pieces that make everything
//! else safe and testable without root:
//!
//! - [`state`]: the durable per-volume state record (V3.2 §15.1) with an
//!   atomic, fsync'd store and a torn-write-tolerant loader.
//! - [`planner`]: the recovery planner — the V3.2 §15.2 matrix as a *pure
//!   function* over (persisted state, local observation, remote observation).
//!
//! The cycle engine, `LocalStack`/`FakeStack`, watchdog, and HTTP client
//! land in subsequent M8 commits (agent design doc §14 task list).

pub mod agentcycle;
pub mod cachedev;
pub mod control;
pub mod cp;
pub mod cycle;
pub mod dm;
#[cfg(feature = "dm")]
pub mod dm_linux;
pub mod dm_mock;
pub mod handoff;
pub mod hostcmd;
pub mod http;
pub mod inproc;
pub mod nodemech;
pub mod planner;
pub mod policy;
pub mod preflight;
pub mod provision;
pub mod stack;
pub mod state;
pub mod thinstack;
pub mod watchdog;

pub use control::{
    CaptureBinding, ControlPlane, CpError, CycleProgress, MixedApply, PreparedInfo, RangeOp,
};
pub use cycle::{
    AgentError, CrashPoints, CycleEngine, CycleKind, PIPELINE_BATCH_BYTES, PIPELINE_MAX_UPLOADS,
};
pub use dm::{
    diff_mappings, dirty_mappings, slot_b_path, DmConfig, DmDeviceInfo, DmOps, DmStack, SlotConfig,
};
#[cfg(feature = "dm")]
pub use dm_linux::{LinuxDmOps, LinuxDmStack};
pub use dm_mock::{MockDmOps, MockEvent};
pub use hostcmd::{HostRunner, Output as CmdOutput, Runner as CmdRunner};
pub use http::{
    control_plane_client, control_plane_client_with, HttpControlPlane, SnapshotSummary,
    VolumeSummary,
};
pub use inproc::InprocControlPlane;
pub use planner::{plan, LocalObservation, RecoveryAction, RemoteObservation, RemoteSnapStatus};
pub use policy::{decide, ExecutorChoice, ReclaimPolicy, ReclaimStrategy, ReclaimTiming};
pub use provision::{
    build as build_thin_stack, observe as observe_thin_stack, teardown as teardown_thin_stack,
    FastLayout, ReadCacheLayout, StackNames, StackState, ThinStackSpec,
};
pub use stack::{
    DeltaRange, FailPoint, FakeStack, LocalStack, ReclaimSets, SnapRole, StackError, SwingHooks,
    SwingStats,
};
pub use state::{
    AgentVolumeState, BaselinePair, LoadedState, LocalSnap, Phase, ServedRequest, StateError,
    StateStore,
};
#[cfg(feature = "dm")]
pub use thinstack::DmThin;
pub use thinstack::ThinStack;
