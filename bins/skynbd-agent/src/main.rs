//! skynbd-agent: production V3.2 snapshot agent CLI (agent design doc
//! §3.1) plus workstation mounts (CSI design §5). Arguments are parsed
//! with bpaf — run `skynbd-agent --help` (or `<command> --help`) for
//! the full usage; the shapes are:
//!
//! Snapshot cycle (config-driven):
//!   run | once | status | state-dump | abort | preflight | debug-delta
//!     | debug-reclaim | debug-enumerate | debug-swing | debug-sweep
//!     | debug-evict
//!     <config.toml> [--volume NAME]
//!
//! Workstation mounts (config-free):
//!   volumes   --api URL [--token-file F]
//!   snapshots <vol> --api URL [--token-file F]
//!   mount     <vol> <dir> [--snapshot S (--ro | --clone NEW)] [--thin]
//!             [--api URL] [--nbd HOST[:PORT]] [--fstype FS] [--tls]
//!             [--tls-hostname H] [--token-file F] [--state-dir D]
//!   unmount   <dir> [--state-dir D]
//!
//! `--thin` delegates to the resident agent's [mounts] control socket
//! (`<state-dir>/ctl.sock`): the daemon stages the CSI-parity
//! publish-only stack and owns its embedded publish cycle.

mod ctl;
mod wsmount;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bpaf::Bpaf;
use serde::Deserialize;
use skynbd_agent::dm::DmConfig;
use skynbd_agent::dm_linux::LinuxDmStack as DmStack;
use skynbd_agent::stack::LocalStack;
use skynbd_agent::{CycleEngine, HttpControlPlane, StateStore};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    server: String,
    token_path: Option<String>,
    state_dir: PathBuf,
    #[serde(rename = "volume", default)]
    volumes: Vec<VolumeCfg>,
    /// `[mounts]`: serve CSI-parity workstation mounts over a local
    /// control socket while `run` is resident (the CLI's `mount --thin`
    /// delegates here). Absent = the socket is not bound.
    #[serde(default)]
    mounts: Option<MountsCfg>,
}

/// The `[mounts]` section — the node-level knobs the CSI driver takes
/// from its environment, spelled as config for the systemd agent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MountsCfg {
    /// Control socket path (default `<state_dir>/ctl.sock`).
    #[serde(default)]
    socket: Option<PathBuf>,
    /// NBD server host for attaches (or use `nbd_unix`).
    #[serde(default)]
    nbd_host: Option<String>,
    #[serde(default = "default_nbd_port")]
    nbd_port: u16,
    /// Node-local NBD Unix socket (mutually exclusive with `nbd_host`;
    /// implies no TLS — the socket's file mode is the authorization).
    #[serde(default)]
    nbd_unix: Option<PathBuf>,
    /// Enroll a short-lived mTLS client cert at startup and attach
    /// with forced TLS.
    #[serde(default)]
    tls: bool,
    #[serde(default)]
    tls_hostname: Option<String>,
    /// Lease-holder identity for lineage serves (Invariant F).
    /// Default: `/etc/hostname`.
    #[serde(default)]
    node_id: Option<String>,
    /// Per-volume pool file directory (default `<state_dir>/cache`).
    #[serde(default)]
    cache_dir: Option<PathBuf>,
    /// Pool file size per volume.
    #[serde(default = "default_cache_bytes")]
    cache_bytes: u64,
    /// §7.1 top-of-stack read cache: per-volume rcache file directory.
    /// Setting it is the opt-in (CSI parity: `SKYNBD_READ_CACHE_DIR`);
    /// absent = no read cache, the filesystem mounts the shim. The
    /// file's metadata is the restage warmth; a stage that rebuilds
    /// the pool fresh zeroes it (fresh pool ⇒ cold cache).
    #[serde(default)]
    read_cache_dir: Option<PathBuf>,
    /// Per-volume read-cache file size.
    #[serde(default = "default_read_cache_bytes")]
    read_cache_bytes: u64,
    #[serde(default = "default_chunk")]
    chunk_bytes: u64,
    /// Publish cadence for mounted volumes' embedded cycles.
    #[serde(default = "default_interval")]
    interval_s: u64,
    #[serde(default = "default_budget")]
    freeze_budget_ms: u64,
    /// §4 `advancePoolWatermark` for mounted volumes (CSI parity:
    /// `SKYNBD_ADVANCE_POOL_WATERMARK`): pool-occupancy fraction above
    /// which a publish cycle is promoted to an advance — the reclaim
    /// trigger for a long-lived mount whose pool would otherwise only
    /// grow. Absent = never (publishes only), the pre-15e posture.
    #[serde(default, alias = "advancePoolWatermark")]
    advance_watermark: Option<f64>,
    /// §7 node-level compaction dials (CSI parity: `SKYNBD_RECLAIM_*`),
    /// applied to every mounted volume's embedded cycle. Same engine,
    /// same dials, same refusals as the StorageClass surface.
    #[serde(default, alias = "reclaimStrategy")]
    reclaim_strategy: Option<String>,
    #[serde(default, alias = "reclaimTiming")]
    reclaim_timing: Option<String>,
    #[serde(default, alias = "reclaimRangeThreshold")]
    reclaim_range_threshold: Option<usize>,
    #[serde(default, alias = "swingEveryNAdvances")]
    reclaim_swing_every_n: Option<u32>,
    /// §8.1 rule 1's window floor in seconds (CSI parity:
    /// `SKYNBD_ADVANCE_WINDOW_SECONDS`): the span of the rescue advance
    /// the watermark's margin reserves write-rate headroom for. Absent =
    /// the engine's 30 s default. A floor, never a ceiling — an advance
    /// observed to take longer raises it. Only bites under a configured
    /// `advance_watermark`, matching the engine's own gating.
    #[serde(default, alias = "advanceWindowSeconds")]
    advance_window_s: Option<u64>,
    /// §6.2 `publishRetentionWindow` in seconds (task 15f; CSI parity:
    /// `SKYNBD_PUBLISH_RETENTION_WINDOW_SECONDS`): arm the post-advance
    /// retention pass, run server-side under this node's fence epoch.
    /// Absent = no thinning from this node.
    #[serde(default, alias = "publishRetentionWindowSeconds")]
    retention_window_s: Option<u64>,
    /// Backfill upload batch depth (issue #97; CSI parity:
    /// `SKYNBD_PUBLISH_PIPELINE_DEPTH`): ops whose objects upload
    /// together before the batch attaches in order. Absent = 1, one op
    /// at a time.
    #[serde(default, alias = "publishPipelineDepth")]
    publish_pipeline_depth: Option<usize>,
    /// Whether a mixed attach/clear batch goes through the server's
    /// single `apply` call (CSI parity: `SKYNBD_MIXED_APPLY`): `auto` (the
    /// default) as the server's `x-skynbd-features` header says,
    /// `always` for a proxy that strips that header in front of a server
    /// that has the call (an old server still works: a refusal without the
    /// header splits the rest of the cycle), `never` to split every mixed
    /// batch.
    #[serde(default, alias = "mixedApply")]
    mixed_apply: Option<skynbd_agent::MixedApply>,
    /// Above a pipeline depth of 1, stage batch k+1 while batch k attaches
    /// (CSI parity: `SKYNBD_PUBLISH_OVERLAP`). Absent = true. False keeps
    /// the batching but stages each batch only after the previous one has
    /// attached — the escape hatch if overlap ever misbehaves against a
    /// server, without dropping to depth 1.
    #[serde(default, alias = "publishOverlap")]
    publish_overlap: Option<bool>,
    /// §8.1 rule 1's flat reserve for the dirty data fsfreeze flushes
    /// into the pool (CSI parity: `SKYNBD_DIRTY_FLUSH_RESERVE_BYTES`).
    /// Absent = the engine's 64 MiB default.
    #[serde(default, alias = "dirtyFlushReserveBytes")]
    dirty_flush_reserve_bytes: Option<u64>,
    /// §8.1 rule 3 for mounted volumes (CSI parity:
    /// `SKYNBD_PUBLISH_FAILING_RO_*`): once publishes have been failing
    /// for `publish_failing_ro_seconds` AND the pool is at or above
    /// this occupancy, take the filesystem read-only rather than let it
    /// exhaust into a dirty shutdown. Both keys must be set together —
    /// one term is not the rule, and defaulting the missing half would
    /// arm a destructive action from a partial configuration (the same
    /// both-or-neither `validate` enforces for `[[volume]]`). Absent =
    /// off, the only safe default: a deliberate `EROFS` stops the
    /// workload.
    #[serde(default, alias = "publishFailingReadOnlyWatermark")]
    publish_failing_ro_watermark: Option<f64>,
    #[serde(default, alias = "publishFailingReadOnlySeconds")]
    publish_failing_ro_seconds: Option<u64>,
}

fn default_nbd_port() -> u16 {
    10809
}
fn default_cache_bytes() -> u64 {
    1 << 30
}
fn default_read_cache_bytes() -> u64 {
    256 << 20
}

/// `deny_unknown_fields` because the §4/§8 dials have two names each —
/// the camelCase one the design doc uses and the snake_case one TOML
/// takes — and serde silently drops what it does not recognise. An
/// operator writing `advancePoolWatermark` (the ONLY name §4 gives that
/// knob) got a config that parsed cleanly and armed nothing: the exact
/// silent half-arming rule 3's both-or-neither check exists to prevent,
/// reached by the likelier route. The `alias`es below accept the doc
/// spellings; anything else is now a startup error rather than a knob
/// that quietly does nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeCfg {
    name: String,
    pool_name: String,
    origin_name: String,
    origin_dev_id: u64,
    metadata_dev: PathBuf,
    mountpoint: PathBuf,
    size_bytes: u64,
    #[serde(default = "default_chunk")]
    chunk_bytes: u64,
    #[serde(default = "default_interval")]
    interval_s: u64,
    #[serde(default = "default_budget")]
    freeze_budget_ms: u64,
    /// dm-linear discard port for live reclaims (D-M15); absent =
    /// discard the origin directly (unmounted rigs only).
    #[serde(default)]
    discard_name: Option<String>,
    /// The file whose filesystem bounds this pool's growth — §8.1 rule
    /// 2's second exhaustion surface. Independent of `[volume.slot]`;
    /// absent = the pool sits on a raw device, so pressure is pool
    /// occupancy alone.
    #[serde(default)]
    backing_file: Option<PathBuf>,
    /// The dm-linear the guest filesystem claims — where a guest
    /// `fstrim` enters, and so where Invariant B's mask sits and is
    /// re-asserted every cycle. Absent = nothing mounts this stack
    /// (the unmounted rig posture), so there is no guest to trim.
    /// `[volume.slot].shim_name` answers for slot-managed stacks.
    #[serde(default)]
    shim_name: Option<String>,
    /// The guest-visible mount device when a §7.1 read cache sits over
    /// the shim (the dm-cache device name). The cache advertises its
    /// own discard limits — the shim's mask is invisible through it —
    /// so the per-cycle re-assert must also land here. Absent = the
    /// shim is the mount device (no read cache).
    #[serde(default)]
    mount_name: Option<String>,
    /// The managed pool slot (`[volume.slot]`, §7): this stack owns its
    /// pool's backing file, so the two-pool swing is supported. Absent =
    /// swing unsupported; reclaims stay on the port.
    #[serde(default)]
    slot: Option<SlotCfg>,
    /// `advancePoolWatermark` (§4): pool-occupancy fraction above which
    /// a publish cycle is promoted to an advance. Absent = never, for
    /// the occupancy term AND for §8.1 rule 1's margin — a deployment
    /// that never opted into watermark advances must not start getting
    /// them from the margin rule.
    #[serde(default, alias = "advancePoolWatermark")]
    advance_watermark: Option<f64>,
    /// §8.1 rule 1's window floor in seconds: the span of the rescue
    /// advance the margin reserves write-rate headroom for. Absent =
    /// the engine's 30 s default. A floor, never a ceiling — an advance
    /// observed to take longer raises it.
    #[serde(default, alias = "advanceWindowSeconds")]
    advance_window_s: Option<u64>,
    /// §6.2 `publishRetentionWindow` in seconds (task 15f; CSI parity:
    /// `SKYNBD_PUBLISH_RETENTION_WINDOW_SECONDS`): arm the post-advance
    /// retention pass, run server-side under this node's fence epoch.
    /// Absent = no thinning from this node.
    #[serde(default, alias = "publishRetentionWindowSeconds")]
    retention_window_s: Option<u64>,
    /// Backfill upload batch depth (issue #97; CSI parity:
    /// `SKYNBD_PUBLISH_PIPELINE_DEPTH`): ops whose objects upload
    /// together before the batch attaches in order. Absent = 1, one op
    /// at a time.
    #[serde(default, alias = "publishPipelineDepth")]
    publish_pipeline_depth: Option<usize>,
    /// Whether a mixed attach/clear batch goes through the server's
    /// single `apply` call (CSI parity: `SKYNBD_MIXED_APPLY`): `auto` (the
    /// default) as the server's `x-skynbd-features` header says,
    /// `always` for a proxy that strips that header in front of a server
    /// that has the call (an old server still works: a refusal without the
    /// header splits the rest of the cycle), `never` to split every mixed
    /// batch.
    #[serde(default, alias = "mixedApply")]
    mixed_apply: Option<skynbd_agent::MixedApply>,
    /// Above a pipeline depth of 1, stage batch k+1 while batch k attaches
    /// (CSI parity: `SKYNBD_PUBLISH_OVERLAP`). Absent = true. False keeps
    /// the batching but stages each batch only after the previous one has
    /// attached — the escape hatch if overlap ever misbehaves against a
    /// server, without dropping to depth 1.
    #[serde(default, alias = "publishOverlap")]
    publish_overlap: Option<bool>,
    /// §8.1 rule 1's flat reserve for the dirty data fsfreeze flushes
    /// into the pool. Absent = the engine's 64 MiB default.
    #[serde(default, alias = "dirtyFlushReserveBytes")]
    dirty_flush_reserve_bytes: Option<u64>,
    /// §8.1 rule 3 (`publishFailingReadOnlyWatermark`): once publishes
    /// have been failing for `publish_failing_ro_seconds` AND the pool
    /// is at or above this occupancy, take the filesystem read-only
    /// rather than let it exhaust into a dirty shutdown.
    ///
    /// Both keys must be set together — one term is not the rule, and
    /// defaulting the missing half would arm a destructive action from
    /// a partial configuration. Absent = off, which is the only safe
    /// default: a deliberate `EROFS` stops the workload.
    #[serde(default, alias = "publishFailingReadOnlyWatermark")]
    publish_failing_ro_watermark: Option<f64>,
    #[serde(default, alias = "publishFailingReadOnlySeconds")]
    publish_failing_ro_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotCfg {
    /// The live slot's backing file (canonical name; the B side is
    /// `<live_file>.b` for the swing's duration).
    live_file: PathBuf,
    /// Pool data region size in bytes.
    data_bytes: u64,
    /// Pool tmeta region size in bytes (carved at offset 0).
    meta_bytes: u64,
    /// The dm-linear the filesystem claims — the swing's retarget target.
    shim_name: String,
    /// Device path of the thin device's external origin (both slots
    /// share it; swings never touch it).
    #[serde(default)]
    external_origin: Option<String>,
}

fn default_chunk() -> u64 {
    65536
}
fn default_interval() -> u64 {
    900
}
fn default_budget() -> u64 {
    5000
}

/// Command line (bpaf): the config-driven snapshot-cycle commands and
/// the config-free workstation mount commands (CSI design §5).
#[derive(Debug, Clone, Bpaf)]
struct CfgArgs {
    /// Volume to operate on (defaults to the only configured one)
    #[bpaf(long, argument("NAME"))]
    volume: Option<String>,
    /// Agent config file
    #[bpaf(positional("CONFIG"))]
    config: String,
}

#[derive(Debug, Clone, Bpaf)]
struct MountArgs {
    /// Control-plane URL, e.g. http://server:8080
    #[bpaf(long, argument("URL"))]
    api: Option<String>,
    /// Bearer token file (re-read per request)
    #[bpaf(long, argument("FILE"))]
    token_file: Option<String>,
    /// NBD endpoint; defaults to the API host on port 10809
    #[bpaf(long, argument("HOST[:PORT]"))]
    nbd: Option<String>,
    /// Published snapshot to mount (pick --ro or --clone)
    #[bpaf(long, argument("NAME"))]
    snapshot: Option<String>,
    /// Mount the snapshot read-only
    #[bpaf(long)]
    ro: bool,
    /// Fast-clone the snapshot into a new writable volume, mount that
    #[bpaf(long("clone"), argument("NEW"))]
    clone_new: Option<String>,
    /// Stage a publish-only thin stack (origin-advance §5) via the
    /// resident agent's control socket — requires `skynbd-agent run`
    /// with a [mounts] config section; the daemon owns the stack and
    /// its publish cycle, so the mount survives this terminal
    #[bpaf(long)]
    thin: bool,
    /// Filesystem type (default ext4)
    #[bpaf(long, argument("FS"))]
    fstype: Option<String>,
    /// Enroll a short-lived NBD mTLS client cert before attaching
    #[bpaf(long)]
    tls: bool,
    /// TLS hostname when it differs from the NBD host
    #[bpaf(long, argument("HOST"))]
    tls_hostname: Option<String>,
    /// Where mount state (and TLS creds) live
    #[bpaf(
        long,
        argument("DIR"),
        fallback(PathBuf::from("/var/lib/skynbd-agent"))
    )]
    state_dir: PathBuf,
    /// Resident agent control socket for `--thin` (default
    /// <state-dir>/ctl.sock; match the daemon's `[mounts].socket`
    /// when it is set)
    #[bpaf(long, argument("PATH"))]
    socket: Option<PathBuf>,
    /// Volume to mount
    #[bpaf(positional("VOLUME"))]
    volume: String,
    /// Mountpoint directory
    #[bpaf(positional("DIR"))]
    dir: PathBuf,
}

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options, descr("skynbd snapshot agent and workstation mount CLI"))]
enum Cmd {
    /// Daemon: run a snapshot cycle per interval
    #[bpaf(command("run"))]
    Run(#[bpaf(external(cfg_args))] CfgArgs),
    /// Exactly one snapshot cycle, then exit
    #[bpaf(command("once"))]
    Once(#[bpaf(external(cfg_args))] CfgArgs),
    /// Durable state, human view
    #[bpaf(command("status"))]
    Status(#[bpaf(external(cfg_args))] CfgArgs),
    /// Durable state as JSON
    #[bpaf(command("state-dump"))]
    StateDump(#[bpaf(external(cfg_args))] CfgArgs),
    /// Abort the in-flight cycle
    #[bpaf(command("abort"))]
    Abort(#[bpaf(external(cfg_args))] CfgArgs),
    /// Placement + chunk checks
    #[bpaf(command("preflight"))]
    Preflight {
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
    },
    /// Print DeltaRanges between two local snapshots (S14a cross-check)
    #[bpaf(command("debug-delta"))]
    DebugDelta {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
        #[bpaf(positional("OLDER"))]
        older: String,
        #[bpaf(positional("NEWER"))]
        newer: String,
    },
    /// Discard origin blocks a capture snapshot maps that are unchanged
    /// since (D-M15 Invariant B; the origin must already be repointed)
    #[bpaf(command("debug-reclaim"))]
    DebugReclaim {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Print the reclaim ranges without freezing or discarding
        #[bpaf(long("dry-run"))]
        dry_run: bool,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
        /// Local capture snapshot (logical name) to reclaim against
        #[bpaf(positional("SNAPSHOT"))]
        snapshot: String,
    },
    /// Print the §7 enumeration against a capture snapshot: reclaim set
    /// R (shared) and copy set D (dirty) from one metadata-snapshot dump
    #[bpaf(command("debug-enumerate"))]
    DebugEnumerate {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
        /// Local capture snapshot (logical name) to enumerate against
        #[bpaf(positional("SNAPSHOT"))]
        snapshot: String,
    },
    /// Run the two-pool swing (§7 major compaction) against a capture
    /// snapshot; needs a [volume.slot] config section (the origin must
    /// be repointed first). Recover a swing this command did not finish
    /// with debug-sweep, never by re-running it
    #[bpaf(command("debug-swing"))]
    DebugSwing {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
        /// Local capture snapshot (logical name) to swing against
        #[bpaf(positional("SNAPSHOT"))]
        snapshot: String,
    },
    /// Recover a crashed debug-swing: the <state_dir>/<volume>.flip
    /// marker is the §7 durable discriminator — present, slot A is
    /// garbage and B is finished; absent, slot B is garbage
    #[bpaf(command("debug-sweep"))]
    DebugSweep {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
    },
    /// Discard up to MAX_RANGES still-shared ranges on both sides
    /// (§7.1 lazy-timing eviction; the origin must be repointed first)
    #[bpaf(command("debug-evict"))]
    DebugEvict {
        /// Volume to operate on (defaults to the only configured one)
        #[bpaf(long, argument("NAME"))]
        volume: Option<String>,
        /// Agent config file
        #[bpaf(positional("CONFIG"))]
        config: String,
        /// Local capture snapshot (logical name) to evict against
        #[bpaf(positional("SNAPSHOT"))]
        snapshot: String,
        /// Cap on shared ranges evicted in this batch
        #[bpaf(positional("MAX_RANGES"))]
        max_ranges: usize,
    },
    /// List volumes on the server
    #[bpaf(command("volumes"))]
    Volumes {
        /// Control-plane URL, e.g. http://server:8080
        #[bpaf(long, argument("URL"))]
        api: String,
        /// Bearer token file (re-read per request)
        #[bpaf(long, argument("FILE"))]
        token_file: Option<String>,
    },
    /// List a volume's published snapshots
    #[bpaf(command("snapshots"))]
    Snapshots {
        /// Control-plane URL, e.g. http://server:8080
        #[bpaf(long, argument("URL"))]
        api: String,
        /// Bearer token file (re-read per request)
        #[bpaf(long, argument("FILE"))]
        token_file: Option<String>,
        /// Volume whose snapshots to list
        #[bpaf(positional("VOLUME"))]
        volume: String,
    },
    /// Mount a volume head, RO snapshot, or fast clone here
    #[bpaf(command("mount"))]
    Mount(#[bpaf(external(mount_args))] MountArgs),
    /// Tear down whatever `mount` built at a directory
    #[bpaf(command("unmount"))]
    Unmount {
        /// Where mount state lives
        #[bpaf(
            long,
            argument("DIR"),
            fallback(PathBuf::from("/var/lib/skynbd-agent"))
        )]
        state_dir: PathBuf,
        /// Resident agent control socket (default <state-dir>/ctl.sock;
        /// match the daemon's `[mounts].socket` when it is set)
        #[bpaf(long, argument("PATH"))]
        socket: Option<PathBuf>,
        /// Mountpoint directory
        #[bpaf(positional("DIR"))]
        dir: PathBuf,
    },
}

fn load_config(path: &str) -> Config {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(66);
    });
    let cfg: Config = toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(65);
    });
    if let Err(e) = validate(&cfg) {
        eprintln!("{e}");
        std::process::exit(78);
    }
    cfg
}

/// §8.1 rule 3 needs BOTH terms or neither. Split from [`load_config`]
/// so a test can reach it: `load_config` ends in `process::exit`, and a
/// test that cannot call the check can only restate its own input —
/// which is coverage that looks like coverage and survives the check
/// being deleted.
///
/// Validated at parse rather than where the dials are applied, because
/// every subcommand loads the config but only `run`/`once` reach the
/// engine setup, so a half-configured rule used to be accepted in
/// silence by `abort`, `preflight` and every `debug-*`. Over all volumes
/// rather than the picked one for the same reason — the refusal is about
/// the file being wrong, and `--volume` should not decide whether it is
/// read.
fn validate(cfg: &Config) -> Result<(), String> {
    for v in &cfg.volumes {
        if v.publish_failing_ro_watermark.is_some() != v.publish_failing_ro_seconds.is_some() {
            return Err(format!(
                "volume {}: publish_failing_ro_watermark and \
                 publish_failing_ro_seconds must be set together (§8.1 rule 3)",
                v.name
            ));
        }
    }
    if let Some(m) = &cfg.mounts {
        // Rule 3's both-or-neither, on the [mounts] surface too — the
        // same partial-configuration hazard as the per-volume pair.
        if m.publish_failing_ro_watermark.is_some() != m.publish_failing_ro_seconds.is_some() {
            return Err("[mounts]: publish_failing_ro_watermark and \
                        publish_failing_ro_seconds must be set together (§8.1 rule 3)"
                .into());
        }
        match (&m.nbd_host, &m.nbd_unix) {
            (None, None) => {
                return Err("[mounts]: one of nbd_host / nbd_unix is required".into());
            }
            (Some(_), Some(_)) => {
                return Err("[mounts]: nbd_host and nbd_unix are mutually exclusive".into());
            }
            // A Unix data path carries no peer certificate, so mTLS has
            // nothing to bind to (same refusal as the CSI driver): do
            // not silently downgrade an explicit tls = true.
            (None, Some(_)) if m.tls => {
                return Err("[mounts]: nbd_unix and tls = true are incompatible: a \
                            Unix data path is guarded by the socket's file mode, \
                            not by client certificates"
                    .into());
            }
            _ => {}
        }
    }
    if cfg.volumes.is_empty() && cfg.mounts.is_none() {
        return Err("config has no [[volume]] and no [mounts]: nothing to run".into());
    }
    Ok(())
}

fn pick<'a>(cfg: &'a Config, volume: Option<&str>) -> &'a VolumeCfg {
    match volume {
        Some(v) => cfg.volumes.iter().find(|c| c.name == v).unwrap_or_else(|| {
            eprintln!("unknown volume {v:?}");
            std::process::exit(64);
        }),
        None if cfg.volumes.len() == 1 => &cfg.volumes[0],
        None => {
            eprintln!("multiple volumes configured; pass --volume");
            std::process::exit(64);
        }
    }
}

fn build(cfg: &Config, v: &VolumeCfg) -> (DmStack, HttpControlPlane, StateStore) {
    // Fail fast on an unreadable token file; afterwards it is re-read per
    // request so rotated (projected) tokens are picked up live.
    if let Some(p) = &cfg.token_path {
        if let Err(e) = std::fs::read_to_string(p) {
            eprintln!("read token {p}: {e}");
            std::process::exit(66);
        }
    }
    let stack = DmStack::new_linux(DmConfig {
        pool_name: v.pool_name.clone(),
        origin_name: v.origin_name.clone(),
        origin_dev_id: v.origin_dev_id,
        metadata_dev: v.metadata_dev.clone(),
        mountpoint: v.mountpoint.clone(),
        volume: v.name.clone(),
        size_bytes: v.size_bytes,
        chunk_bytes: v.chunk_bytes,
        ids_path: cfg.state_dir.join(format!("{}.devids", v.name)),
        freeze_budget_ms: v.freeze_budget_ms,
        discard_name: v.discard_name.clone(),
        backing_file: v.backing_file.clone(),
        shim_name: v.shim_name.clone(),
        mount_name: v.mount_name.clone(),
        slot: v.slot.as_ref().map(|s| skynbd_agent::SlotConfig {
            live_file: s.live_file.clone(),
            data_bytes: s.data_bytes,
            meta_bytes: s.meta_bytes,
            shim_name: s.shim_name.clone(),
            external_origin: s.external_origin.clone(),
            // Config rigs build their own A side under the `-tm`/`-da`
            // convention; provisioned stacks are the ones that override.
            tmeta_name: None,
            data_name: None,
        }),
    })
    .unwrap_or_else(|e| {
        eprintln!("dm stack: {e}");
        std::process::exit(74);
    });
    let cp = match &cfg.token_path {
        Some(p) => HttpControlPlane::with_token_file(cfg.server.clone(), v.name.clone(), p),
        None => HttpControlPlane::new(cfg.server.clone(), v.name.clone(), None),
    }
    .with_mixed_apply(v.mixed_apply.unwrap_or_default());
    let store = StateStore::new(&cfg.state_dir);
    (stack, cp, store)
}

fn preflight(cfg: &Config) -> i32 {
    let mut rc = 0;
    let mounts: Vec<&std::path::Path> =
        cfg.volumes.iter().map(|v| v.mountpoint.as_path()).collect();
    match skynbd_agent::preflight::check_placement(&cfg.state_dir, &mounts) {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("warn: {w}");
            }
            for f in &r.failures {
                eprintln!("FAIL: {f}");
                rc = 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL: preflight io: {e}");
            rc = 1;
        }
    }
    // §8.1 exhaustion-ladder posture: not a rule, a fact the operator
    // has to know per rig — what queue mode costs on this kernel, and
    // whether it outlasts the freeze budget the rescue advance runs
    // under.
    for v in &cfg.volumes {
        let r = skynbd_agent::preflight::check_no_space_timeout(v.freeze_budget_ms);
        for n in &r.notes {
            println!("note[{}]: {n}", v.name);
        }
        for w in &r.warnings {
            eprintln!("warn[{}]: {w}", v.name);
        }
    }
    for v in &cfg.volumes {
        let r = skynbd_agent::preflight::check_chunk(v.chunk_bytes, v.chunk_bytes);
        // Pool block size is verified against the live pool in the rig;
        // config-level check is the divisor rule placeholder.
        for f in &r.failures {
            eprintln!("FAIL[{}]: {f}", v.name);
            rc = 1;
        }
    }
    if rc == 0 {
        println!("preflight OK");
    }
    rc
}

/// Enroll a short-lived NBD mTLS client cert into `dir` (auth design
/// §3.2): keygen locally, CSR to the control plane, PEMs written where
/// nbd-client reads them. Same contract as the CSI node's creds dir.
async fn enroll_creds(cp: &HttpControlPlane, dir: &std::path::Path) -> Result<(), String> {
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let csr = rcgen::CertificateParams::new(Vec::<String>::new())
        .map_err(|e| e.to_string())?
        .serialize_request(&key)
        .map_err(|e| e.to_string())?
        .pem()
        .map_err(|e| e.to_string())?;
    let issued = cp.enroll_cert(&csr).await.map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let write = |name: &str, data: &str, mode: u32| -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())
    };
    write("client-key.pem", &key.serialize_pem(), 0o600)?;
    write("client-cert.pem", &issued.cert_pem, 0o644)?;
    write("ca-cert.pem", &issued.ca_pem, 0o644)?;
    eprintln!(
        "enrolled as {} (valid until {})",
        issued.identity, issued.not_after
    );
    Ok(())
}

/// Build the mount daemon from `[mounts]` (mirroring what the CSI node
/// builds from its environment), replay recorded mounts, and serve the
/// control socket as a background task. Exits the process on
/// construction failures — a `[mounts]` section that cannot serve is a
/// startup error, not a degraded mode.
async fn serve_mounts(cfg: &Config, m: &MountsCfg) {
    use skynbd_agent::{agentcycle, cp, handoff, nodemech, thinstack};

    let token_file = cfg.token_path.clone().map(PathBuf::from);
    let creds_dir = cfg.state_dir.join("creds");
    if m.tls {
        let cp = make_cp(&cfg.server, &cfg.token_path, "-");
        if let Err(e) = enroll_creds(&cp, &creds_dir).await {
            eprintln!("[mounts] enroll: {e}");
            std::process::exit(74);
        }
    }
    let endpoint = match (&m.nbd_unix, &m.nbd_host) {
        (Some(p), _) => nodemech::NbdEndpoint::Unix(p.clone()),
        (None, Some(h)) => nodemech::NbdEndpoint::Tcp {
            host: h.clone(),
            port: m.nbd_port,
            tls_hostname: m.tls_hostname.clone(),
            creds_dir: m.tls.then(|| creds_dir.clone()),
        },
        (None, None) => unreachable!("validate() requires one"),
    };
    let node_cfg = nodemech::NodeConfig {
        endpoint,
        sysfs_block: "/sys/block".into(),
        state_dir: cfg.state_dir.join("mount-state"),
        default_fstype: "ext4".into(),
        cache_dir: Some(
            m.cache_dir
                .clone()
                .unwrap_or_else(|| cfg.state_dir.join("cache")),
        ),
        cache_bytes: m.cache_bytes,
        chunk_bytes: m.chunk_bytes,
        read_cache_dir: m.read_cache_dir.clone(),
        read_cache_bytes: m.read_cache_bytes,
    };
    let thin = thinstack::DmThin::new().unwrap_or_else(|e| {
        eprintln!("[mounts] device-mapper: {e}");
        std::process::exit(74);
    });
    let mut cycle = agentcycle::DmCycle::new(
        cfg.server.clone(),
        token_file.clone(),
        cfg.state_dir.join("agent"),
        m.chunk_bytes,
        Duration::from_secs(m.interval_s),
        m.freeze_budget_ms,
    );
    // §4 watermark + §7 compaction dials (CSI parity, task 15e): the
    // same knobs the CSI node takes from its environment, spelled as
    // `[mounts]` config. A bad value is a startup error (EX_CONFIG),
    // like every other misconfigured section.
    cycle.advance_watermark = m.advance_watermark;
    fn dial<T>(what: &str, r: Result<T, String>) -> T {
        r.unwrap_or_else(|e| {
            eprintln!("[mounts] {what}: {e}");
            std::process::exit(78);
        })
    }
    if let Some(s) = &m.reclaim_strategy {
        cycle.policy.strategy = dial("reclaim_strategy", s.parse());
    }
    if let Some(t) = &m.reclaim_timing {
        cycle.policy.timing = dial("reclaim_timing", t.parse());
    }
    if let Some(r) = m.reclaim_range_threshold {
        cycle.policy.range_threshold = r;
    }
    if let Some(n) = m.reclaim_swing_every_n {
        // Through the shared helper so 0 is refused with the same
        // words as the StorageClass surface.
        cycle.policy.swing_every_n = Some(dial(
            "swing_every_n",
            agentcycle::parse_swing_every_n(&n.to_string()),
        ));
    }
    // §8.1 rule 1's margin dials — both only bite under a configured
    // watermark, the engine's own gating.
    cycle.advance_window = m.advance_window_s.map(Duration::from_secs);
    cycle.dirty_flush_reserve = m.dirty_flush_reserve_bytes;
    // §6.2 retention (task 15f): the node drives the pass because the
    // daemon's own cadence is fenced off volumes this node leases.
    cycle.retention_window_s = m.retention_window_s;
    // Issue #97: backfill upload pipelining. Absent = depth 1.
    cycle.publish_pipeline_depth = m.publish_pipeline_depth;
    // Issue #97 follow-up: whether mixed batches use `apply`.
    cycle.mixed_apply = m.mixed_apply;
    cycle.publish_overlap = m.publish_overlap;
    // §8.1 rule 3. `validate` already refused a half-configured pair at
    // startup, so zip is exact here — both or neither.
    cycle.ro_watermark = m
        .publish_failing_ro_watermark
        .zip(m.publish_failing_ro_seconds)
        .map(|(w, s)| (w, Duration::from_secs(s)));
    // Invariant F: the lease-holder identity for lineage serves.
    let node_id = match &m.node_id {
        Some(id) => id.clone(),
        None => std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "workstation".into()),
    };
    let client = || cp::Client::new(&cfg.server, token_file.clone());
    let mechanics = Arc::new(
        nodemech::NodeMechanics::new(
            node_cfg,
            Arc::new(skynbd_agent::hostcmd::HostRunner),
            Some(Arc::new(thin) as Arc<dyn thinstack::ThinStack>),
            Some(Arc::new(cycle) as Arc<dyn agentcycle::CycleRunner>),
        )
        .with_handoff(Arc::new(handoff::CpHandoff::new(client(), node_id)))
        .with_origins(Arc::new(handoff::CpOrigins::new(client()))),
    );
    if let Err(e) = std::fs::create_dir_all(&cfg.state_dir) {
        eprintln!("[mounts] create {}: {e}", cfg.state_dir.display());
        std::process::exit(66);
    }
    let mounts = Arc::new(ctl::Mounts::new(mechanics, client(), &cfg.state_dir));
    // INVARIANT: replay completes before the socket binds. No request
    // can observe (or race) a half-replayed table — a `mount` for a dir
    // whose replay is still staging would double-stage, and a `status`
    // would report mounts absent that are about to come back. Swapping
    // these two lines would look harmless; it is not.
    mounts.replay().await;
    let socket = m
        .socket
        .clone()
        .unwrap_or_else(|| cfg.state_dir.join("ctl.sock"));
    let listener = ctl::bind(&socket).unwrap_or_else(|e| {
        eprintln!("[mounts] bind {}: {e}", socket.display());
        std::process::exit(74);
    });
    tracing::info!(socket = %socket.display(), "mount control socket serving");
    tokio::spawn(ctl::serve(listener, mounts));
}

fn make_cp(api: &str, token_file: &Option<String>, volume: &str) -> HttpControlPlane {
    match token_file {
        Some(p) => HttpControlPlane::with_token_file(api, volume, p),
        None => HttpControlPlane::new(api, volume, None),
    }
}

async fn ws_volumes(api: String, token_file: Option<String>) -> i32 {
    match make_cp(&api, &token_file, "").list_volumes().await {
        Ok(vols) => {
            for v in vols {
                println!("{}\t{}", v.name, v.size);
            }
            0
        }
        Err(e) => {
            eprintln!("list volumes: {e}");
            74
        }
    }
}

async fn ws_snapshots(api: String, token_file: Option<String>, volume: String) -> i32 {
    match make_cp(&api, &token_file, &volume).list_snapshots().await {
        Ok(snaps) => {
            for s in snaps {
                println!("{}\t{}", s.name, s.commit);
            }
            0
        }
        Err(e) => {
            eprintln!("list snapshots: {e}");
            74
        }
    }
}

/// The `mount` subcommand (CSI design §5). Returns the exit code.
async fn ws_mount(a: MountArgs) -> i32 {
    let cp = |volume: &str| -> HttpControlPlane {
        let Some(api) = &a.api else {
            eprintln!("--api URL is required for this operation");
            std::process::exit(64);
        };
        make_cp(api, &a.token_file, volume)
    };
    // The three mount shapes (design §5): plain head, RO snapshot, or
    // fast clone. --snapshot needs one of --ro / --clone: mounting a
    // snapshot writably in place is not a thing SkyNBD offers (that's
    // what clones are for).
    let (mount_vol, kind) = match (&a.snapshot, &a.clone_new, a.ro) {
        (None, None, false) => (a.volume.clone(), wsmount::MountKind::Head),
        (Some(s), None, true) => (a.volume.clone(), wsmount::MountKind::RoSnapshot(s.clone())),
        (Some(s), Some(new), false) => {
            // Server fast clone (design §4.1), then mount the clone's
            // head like any other volume.
            let from = format!("{}/{s}", a.volume);
            match cp(&a.volume).clone_volume(new, &from).await {
                Ok(v) => {
                    eprintln!("cloned {from} -> {} ({} bytes)", v.name, v.size);
                    (v.name, wsmount::MountKind::Head)
                }
                Err(e) => {
                    eprintln!("clone: {e}");
                    return 74;
                }
            }
        }
        (Some(_), None, false) => {
            eprintln!("--snapshot needs --ro (read-only mount) or --clone NEW (writable copy)");
            return 64;
        }
        (Some(_), Some(_), true) => {
            eprintln!("--ro and --clone are mutually exclusive");
            return 64;
        }
        (None, _, _) => {
            eprintln!("--clone/--ro need --snapshot");
            return 64;
        }
    };
    // --thin delegates to the resident agent (its [mounts] control
    // socket): the daemon owns the publish-only stack AND the embedded
    // publish cycle that is its durability path — the same layout and
    // code the CSI node stages, surviving this terminal.
    if a.thin {
        if a.ro {
            eprintln!("--thin applies to writable mounts (clone the snapshot instead)");
            return 64;
        }
        let sock = a
            .socket
            .clone()
            .unwrap_or_else(|| a.state_dir.join("ctl.sock"));
        let req = ctl::Request::Mount {
            volume: mount_vol.clone(),
            dir: a.dir.display().to_string(),
            fstype: a.fstype.clone(),
        };
        return match ctl::call(&sock, &req).await {
            Ok(r) if r.ok => {
                println!(
                    "{mount_vol} on {} (agent-managed publish-only thin)",
                    a.dir.display()
                );
                0
            }
            Ok(r) => {
                eprintln!(
                    "mount: {}",
                    r.error.unwrap_or_else(|| "agent refused".into())
                );
                74
            }
            Err(e) => {
                eprintln!("mount: {e}");
                74
            }
        };
    }
    // NBD endpoint: explicit --nbd, else the API host on the standard
    // NBD port.
    let (nbd_host, nbd_port) = match &a.nbd {
        Some(hp) => match hp.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse().unwrap_or_else(|_| {
                    eprintln!("bad --nbd port {p:?}");
                    std::process::exit(64);
                }),
            ),
            None => (hp.clone(), 10809),
        },
        None => {
            let Some(api) = &a.api else {
                eprintln!("--nbd HOST[:PORT] (or --api to derive the host) is required");
                std::process::exit(64);
            };
            let host = api
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .split(['/', ':'])
                .next()
                .unwrap_or_default()
                .to_string();
            (host, 10809)
        }
    };
    let creds_dir = if a.tls {
        let dir = a.state_dir.join("creds");
        if let Err(e) = enroll_creds(&cp(&mount_vol), &dir).await {
            eprintln!("enroll: {e}");
            return 74;
        }
        Some(dir)
    } else {
        None
    };
    let w = wsmount::Workstation {
        ctx: wsmount::MountCtx {
            nbd_host,
            nbd_port,
            creds_dir,
            tls_hostname: a.tls_hostname,
            sysfs_block: "/sys/block".into(),
            state_dir: a.state_dir,
            default_fstype: "ext4".into(),
        },
        runner: wsmount::HostRunner,
    };
    match w
        .mount(&mount_vol, &kind, &a.dir, a.fstype.as_deref())
        .await
    {
        Ok(dev) => {
            println!("{mount_vol} on {} ({dev})", a.dir.display());
            0
        }
        Err(e) => {
            eprintln!("mount: {e}");
            74
        }
    }
}

async fn ws_unmount(state_dir: PathBuf, socket: Option<PathBuf>, dir: PathBuf) -> i32 {
    let sock = socket.unwrap_or_else(|| state_dir.join("ctl.sock"));
    let w = wsmount::Workstation {
        ctx: wsmount::MountCtx {
            nbd_host: String::new(),
            nbd_port: 0,
            creds_dir: None,
            tls_hostname: None,
            sysfs_block: "/sys/block".into(),
            state_dir: state_dir.clone(),
            default_fstype: "ext4".into(),
        },
        runner: wsmount::HostRunner,
    };
    match w.unmount(&dir).await {
        Ok(()) => 0,
        // Not a CLI-recorded mount: it may be the resident agent's
        // (`mount --thin` delegates there). Ask before giving up.
        Err(wsmount::MountError::NotMounted(_)) if sock.exists() => {
            let req = ctl::Request::Unmount {
                dir: dir.display().to_string(),
            };
            match ctl::call(&sock, &req).await {
                Ok(r) if r.ok => 0,
                Ok(r) => {
                    eprintln!(
                        "unmount: {}",
                        r.error.unwrap_or_else(|| "agent refused".into())
                    );
                    74
                }
                Err(e) => {
                    eprintln!("unmount: {e}");
                    74
                }
            }
        }
        Err(e) => {
            eprintln!("unmount: {e}");
            74
        }
    }
}

async fn recover_engine(
    stack: DmStack,
    cp: HttpControlPlane,
    store: StateStore,
    v: &VolumeCfg,
) -> CycleEngine<DmStack, HttpControlPlane> {
    let mut eng = CycleEngine::open(stack, cp, store, &v.name).unwrap_or_else(|e| {
        eprintln!("open state: {e}");
        std::process::exit(74);
    });
    // The §4/§8 dials. Each is absent-means-off, and the engine's own
    // defaults cover the two that have them — so a config written
    // before these keys existed behaves exactly as it did.
    if let Some(w) = v.advance_watermark {
        eng = eng.with_advance_watermark(w);
    }
    if let Some(s) = v.advance_window_s {
        eng = eng.with_advance_window(Duration::from_secs(s));
    }
    if let Some(w) = v.retention_window_s {
        eng = eng.with_retention_window(w);
    }
    if let Some(d) = v.publish_pipeline_depth {
        eng = eng.with_publish_pipeline_depth(d);
    }
    if v.mixed_apply == Some(skynbd_agent::MixedApply::Never) {
        eng = eng.with_mixed_apply(false);
    }
    if let Some(o) = v.publish_overlap {
        eng = eng.with_publish_overlap(o);
    }
    if let Some(b) = v.dirty_flush_reserve_bytes {
        eng = eng.with_dirty_flush_reserve(b);
    }
    // §8.1 rule 3. Both-or-neither is enforced at parse (`load_config`),
    // so by here the pair is either whole or absent.
    if let (Some(f), Some(secs)) = (v.publish_failing_ro_watermark, v.publish_failing_ro_seconds) {
        eng = eng.with_ro_watermark(f, Duration::from_secs(secs));
    }
    if let Err(e) = eng.recover().await {
        eprintln!("recovery: {e}");
        std::process::exit(70);
    }
    eng
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Workstation-mount commands (CSI design §5) are config-free; the
    // cycle commands load their TOML config below.
    let (cfg_args, cmd_kind) = match cmd().run() {
        Cmd::Volumes { api, token_file } => std::process::exit(ws_volumes(api, token_file).await),
        Cmd::Snapshots {
            api,
            token_file,
            volume,
        } => std::process::exit(ws_snapshots(api, token_file, volume).await),
        Cmd::Mount(m) => std::process::exit(ws_mount(m).await),
        Cmd::Unmount {
            state_dir,
            socket,
            dir,
        } => std::process::exit(ws_unmount(state_dir, socket, dir).await),
        Cmd::Preflight { config } => {
            let cfg = load_config(&config);
            std::process::exit(preflight(&cfg));
        }
        Cmd::Run(a) => (a, "run"),
        Cmd::Once(a) => (a, "once"),
        Cmd::Status(a) => (a, "status"),
        Cmd::StateDump(a) => (a, "state-dump"),
        Cmd::Abort(a) => (a, "abort"),
        Cmd::DebugDelta {
            volume,
            config,
            older,
            newer,
        } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, _store) = build(&cfg, v);
            match stack.thin_delta(&older, &newer) {
                Ok(ranges) => {
                    for r in ranges {
                        match r {
                            skynbd_agent::DeltaRange::Data { off, len } => {
                                println!("data {off} {len}")
                            }
                            skynbd_agent::DeltaRange::Clear { off, len } => {
                                println!("clear {off} {len}")
                            }
                        }
                    }
                    return;
                }
                Err(e) => {
                    eprintln!("delta: {e}");
                    std::process::exit(70);
                }
            }
        }
        Cmd::DebugReclaim {
            volume,
            dry_run,
            config,
            snapshot,
        } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, _store) = build(&cfg, v);
            if dry_run {
                match stack.reclaim_ranges(&snapshot) {
                    Ok(ranges) => {
                        for (off, len) in ranges {
                            println!("reclaim {off} {len}");
                        }
                        return;
                    }
                    Err(e) => {
                        eprintln!("reclaim ranges: {e}");
                        std::process::exit(70);
                    }
                }
            }
            match stack.reclaim(&snapshot) {
                Ok(bytes) => {
                    println!("reclaimed {bytes} bytes");
                    return;
                }
                Err(e) => {
                    eprintln!("reclaim: {e}");
                    std::process::exit(70);
                }
            }
        }
        Cmd::DebugEnumerate {
            volume,
            config,
            snapshot,
        } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, _store) = build(&cfg, v);
            let t = std::time::Instant::now();
            match stack.enumerate(&snapshot) {
                Ok(sets) => {
                    let shared_bytes: u64 = sets.shared.iter().map(|&(_, len)| len).sum();
                    println!(
                        "enumerated shared_ranges={} shared_bytes={} dirty_ranges={} \
                         dirty_bytes={} ms={}",
                        sets.shared_count(),
                        shared_bytes,
                        sets.dirty.len(),
                        sets.dirty_bytes(),
                        t.elapsed().as_millis(),
                    );
                    return;
                }
                Err(e) => {
                    eprintln!("enumerate: {e}");
                    std::process::exit(70);
                }
            }
        }
        Cmd::DebugSwing {
            volume,
            config,
            snapshot,
        } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, _store) = build(&cfg, v);
            // The durable flip stand-in: what the engine persists in
            // AgentVolumeState, the debug CLI writes as a marker file —
            // same call site (inside the freeze window, after the
            // retarget), same durability (create + fsync, state dir off
            // the frozen fs).
            let flip = cfg.state_dir.join(format!("{}.flip", v.name));
            let mut persist_flip = || -> Result<(), skynbd_agent::StackError> {
                std::fs::File::create(&flip)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| skynbd_agent::StackError::Fault("flip marker write failed"))
            };
            let mut hooks = skynbd_agent::SwingHooks {
                persist_flip: &mut persist_flip,
                retargeted: &mut false,
            };
            match stack.swing(&snapshot, &mut hooks) {
                Ok(s) => {
                    println!(
                        "swing rounds={} copied_bytes={} live_ms={} frozen_ms={}",
                        s.rounds,
                        s.copied_bytes,
                        s.live_ns / 1_000_000,
                        s.frozen_ns / 1_000_000,
                    );
                    return;
                }
                Err(e) => {
                    eprintln!("swing: {e}");
                    std::process::exit(70);
                }
            }
        }
        Cmd::DebugSweep { volume, config } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, store) = build(&cfg, v);
            // §7's discriminator is whatever the durable record says, and
            // there are two writers of it: the engine persists
            // `swing_flipped`, while `debug-swing` — which never touches
            // the agent record — drops a marker file. The record wins
            // when it has an opinion, because guessing the other way
            // destroys live data: an engine swing that died after its
            // flip persisted leaves no marker, and sweeping that as
            // "armed" would put the shim back on the torn-down A side
            // and remove the pool the volume is actually running on.
            let flip = cfg.state_dir.join(format!("{}.flip", v.name));
            let marker = flip.exists();
            let recorded = match store.load(&v.name) {
                Ok(skynbd_agent::LoadedState::Present(s)) => s.swing_flipped,
                _ => None,
            };
            let flipped = recorded.unwrap_or(marker);
            match stack.sweep_slot(flipped) {
                Ok(found) => {
                    // Clear the marker only after the sweep landed: a
                    // failed flipped sweep must stay flipped for the
                    // retry, or the next call would sweep the live side.
                    // The agent record is left to the engine's own
                    // recovery — a debug tool has no business retiring
                    // its crash state.
                    if marker {
                        if let Err(e) = std::fs::remove_file(&flip) {
                            eprintln!("sweep: remove {}: {e}", flip.display());
                            std::process::exit(74);
                        }
                    }
                    println!(
                        "swept flipped={flipped} found={found} source={}",
                        if recorded.is_some() {
                            "record"
                        } else {
                            "marker"
                        }
                    );
                    return;
                }
                Err(e) => {
                    eprintln!("sweep: {e}");
                    std::process::exit(70);
                }
            }
        }
        Cmd::DebugEvict {
            volume,
            config,
            snapshot,
            max_ranges,
        } => {
            let cfg = load_config(&config);
            let v = pick(&cfg, volume.as_deref());
            let (stack, _cp, _store) = build(&cfg, v);
            match stack.evict(&snapshot, max_ranges) {
                Ok(bytes) => {
                    println!("evicted {bytes} bytes");
                    return;
                }
                Err(e) => {
                    eprintln!("evict: {e}");
                    std::process::exit(70);
                }
            }
        }
    };
    let cmd = cmd_kind;
    let volume = cfg_args.volume;
    let cfg = load_config(&cfg_args.config);

    match cmd {
        "status" | "state-dump" => {
            let v = pick(&cfg, volume.as_deref());
            let store = StateStore::new(&cfg.state_dir);
            match store.load(&v.name) {
                Ok(skynbd_agent::LoadedState::Present(s)) => {
                    if cmd == "status" {
                        println!(
                            "volume {}  phase {:?}\n  baseline: {:?}\n  candidate: {:?}\n  prepared: {:?}",
                            s.volume, s.phase, s.baseline, s.candidate, s.prepared
                        );
                    } else {
                        println!("{}", serde_json::to_string_pretty(&*s).unwrap());
                    }
                }
                Ok(skynbd_agent::LoadedState::Absent) => println!("no state for {}", v.name),
                Ok(skynbd_agent::LoadedState::Corrupt) => {
                    println!(
                        "state for {} is corrupt (recovery will reconstruct)",
                        v.name
                    )
                }
                Err(e) => {
                    eprintln!("load: {e}");
                    std::process::exit(74);
                }
            }
        }
        "once" => {
            let v = pick(&cfg, volume.as_deref());
            let (stack, cp, store) = build(&cfg, v);
            let mut eng = recover_engine(stack, cp, store, v).await;
            match eng.run_cycle().await {
                Ok(target) => println!("published {target}"),
                Err(e) => {
                    eprintln!("cycle: {e}");
                    std::process::exit(70);
                }
            }
        }
        "abort" => {
            let v = pick(&cfg, volume.as_deref());
            let (stack, cp, store) = build(&cfg, v);
            let mut eng = CycleEngine::open(stack, cp, store, &v.name).unwrap();
            if let Err(e) = eng.abort_in_flight().await {
                eprintln!("abort: {e}");
                std::process::exit(70);
            }
            println!("aborted");
        }
        "run" => {
            // [mounts]: bind the control socket and replay recorded
            // mounts BEFORE entering the config-volume loop — the
            // mounts' cycles run as their own tasks (DmCycle), so the
            // loop below and the socket serve concurrently.
            if let Some(m) = &cfg.mounts {
                serve_mounts(&cfg, m).await;
            }
            if cfg.volumes.is_empty() {
                // validate() guarantees [mounts] is present here.
                tracing::info!("no [[volume]] configured; serving mounts only");
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("shutting down");
                return;
            }
            let v = pick(&cfg, volume.as_deref());
            let interval = Duration::from_secs(v.interval_s);
            let (stack, cp, store) = build(&cfg, v);
            let mut eng = recover_engine(stack, cp, store, v).await;
            let name = Arc::new(v.name.clone());
            loop {
                match eng.run_cycle().await {
                    Ok(t) => tracing::info!(volume = %name, target = %t, "cycle published"),
                    Err(e) => tracing::warn!(volume = %name, error = %e, "cycle failed"),
                }
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!("shutting down");
                        return;
                    }
                }
            }
        }
        _ => unreachable!("bpaf restricts cmd_kind"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        name = "v"
        pool_name = "p"
        origin_name = "o"
        origin_dev_id = 1
        metadata_dev = "/dev/null"
        mountpoint = "/mnt"
        size_bytes = 1024
    "#;

    /// The design doc's camelCase names parse. §4 gives that first knob
    /// exactly one name — `advancePoolWatermark` — and it is not the
    /// TOML key, so an operator following the doc was writing a key
    /// serde dropped in silence.
    #[test]
    fn the_design_doc_spellings_are_accepted() {
        let v: VolumeCfg = toml::from_str(&format!(
            "{MINIMAL}\n\
             advancePoolWatermark = 0.8\n\
             advanceWindowSeconds = 45\n\
             dirtyFlushReserveBytes = 1024\n\
             publishFailingReadOnlyWatermark = 0.9\n\
             publishFailingReadOnlySeconds = 600\n"
        ))
        .expect("doc spellings should parse");
        assert_eq!(v.advance_watermark, Some(0.8));
        assert_eq!(v.advance_window_s, Some(45));
        assert_eq!(v.dirty_flush_reserve_bytes, Some(1024));
        assert_eq!(v.publish_failing_ro_watermark, Some(0.9));
        assert_eq!(v.publish_failing_ro_seconds, Some(600));
    }

    /// And anything else is a startup error rather than a knob that
    /// quietly does nothing — which is what a near-miss spelling of a
    /// §8 dial used to be.
    #[test]
    fn an_unrecognised_key_is_refused() {
        let e = toml::from_str::<VolumeCfg>(&format!(
            "{MINIMAL}\npublish_failing_readonly_watermark = 0.9\n"
        ))
        .expect_err("an unknown key must not be silently dropped");
        assert!(
            e.to_string().contains("unknown field"),
            "unhelpful error: {e}"
        );
    }

    fn config_with(dials: &str) -> Config {
        toml::from_str(&format!(
            "server = \"http://127.0.0.1:8080\"\n\
             state_dir = \"/var/lib/skynbd-agent\"\n\
             [[volume]]\n{MINIMAL}\n{dials}"
        ))
        .expect("fixture should parse")
    }

    /// Rule 3's pair is both-or-neither, asserted through `validate` —
    /// the function `load_config` exits 78 on. Deleting the check must
    /// fail this test. An earlier version asserted
    /// `half.a.is_some() != half.b.is_some()`, which merely restated its
    /// own input and passed happily with the check stubbed out.
    #[test]
    fn rule_3_needs_both_terms_or_neither() {
        let err = validate(&config_with("publish_failing_ro_watermark = 0.9\n"))
            .expect_err("a watermark with no duration must be refused");
        assert!(err.contains("must be set together"), "unhelpful: {err}");

        let err = validate(&config_with("publish_failing_ro_seconds = 600\n"))
            .expect_err("a duration with no watermark must be refused");
        assert!(err.contains("must be set together"), "unhelpful: {err}");

        validate(&config_with(
            "publish_failing_ro_watermark = 0.9\npublish_failing_ro_seconds = 600\n",
        ))
        .expect("both terms is the armed rule");
        validate(&config_with("")).expect("neither term is off, which is the default");
    }

    fn mounts_config_with(dials: &str) -> Config {
        toml::from_str(&format!(
            "server = \"http://127.0.0.1:8080\"\n\
             state_dir = \"/var/lib/skynbd-agent\"\n\
             [mounts]\nnbd_host = \"127.0.0.1\"\n{dials}"
        ))
        .expect("fixture should parse")
    }

    /// The `[mounts]` §8.1 dials landed with the same two properties as
    /// the per-volume ones: the design doc's camelCase spellings parse,
    /// and rule 3's pair is both-or-neither through the same `validate`
    /// that `load_config` exits 78 on.
    #[test]
    fn mounts_hardening_dials_parse_and_rule_3_needs_both_terms() {
        let cfg = mounts_config_with(
            "advanceWindowSeconds = 45\n\
             dirtyFlushReserveBytes = 1024\n\
             publishRetentionWindowSeconds = 3600\n\
             publishPipelineDepth = 16\n\
             mixedApply = \"always\"\n\
             publishOverlap = false\n\
             publishFailingReadOnlyWatermark = 0.9\n\
             publishFailingReadOnlySeconds = 600\n",
        );
        let m = cfg.mounts.as_ref().unwrap();
        assert_eq!(m.advance_window_s, Some(45));
        assert_eq!(m.retention_window_s, Some(3600));
        assert_eq!(m.publish_pipeline_depth, Some(16));
        assert_eq!(m.mixed_apply, Some(skynbd_agent::MixedApply::Always));
        assert_eq!(m.publish_overlap, Some(false));
        assert_eq!(m.dirty_flush_reserve_bytes, Some(1024));
        assert_eq!(m.publish_failing_ro_watermark, Some(0.9));
        assert_eq!(m.publish_failing_ro_seconds, Some(600));
        validate(&cfg).expect("both terms is the armed rule");

        let err = validate(&mounts_config_with("publish_failing_ro_watermark = 0.9\n"))
            .expect_err("a watermark with no duration must be refused");
        assert!(err.contains("must be set together"), "unhelpful: {err}");
        let err = validate(&mounts_config_with("publish_failing_ro_seconds = 600\n"))
            .expect_err("a duration with no watermark must be refused");
        assert!(err.contains("must be set together"), "unhelpful: {err}");
        validate(&mounts_config_with("")).expect("neither term is off, which is the default");
    }

    /// The refusal is per-file, not per-`--volume`: a later volume's
    /// half-configured rule is still the file being wrong.
    #[test]
    fn every_volume_is_validated_not_just_the_first() {
        let cfg: Config = toml::from_str(&format!(
            "server = \"http://127.0.0.1:8080\"\n\
             state_dir = \"/var/lib/skynbd-agent\"\n\
             [[volume]]\n{MINIMAL}\n\
             [[volume]]\n{MINIMAL}\n\
             publish_failing_ro_seconds = 600\n"
        ))
        .unwrap();
        assert!(
            validate(&cfg).is_err(),
            "a half-configured rule on the second volume must be seen"
        );
    }
}
