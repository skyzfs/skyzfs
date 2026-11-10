//! Resident-agent mount control (CSI-parity workstation mounts).
//!
//! The systemd agent already owns the snapshot machinery; a `[mounts]`
//! config section adds a root-only Unix control socket the CLI's
//! `mount --thin` delegates to, so a workstation thin volume gets the
//! SAME publish-only stack and embedded publish cycle a CSI-staged
//! volume gets — one implementation ([`skynbd_agent::nodemech`]), two
//! front ends. The daemon is the durability owner: it survives the
//! mounting terminal, restarts cycles across its own restarts (the
//! records file is replayed at startup; a live dm stack short-circuits
//! the re-stage), and runs the D-M13 final publish cycle on unmount.
//!
//! Protocol: one JSON request line per connection, one JSON reply
//! line. Authorization is the socket's file mode (0700 parent — the
//! same posture as the CSI driver's node-local NBD Unix socket).
//!
//! Lineage naming follows CSI exactly: volume `v` publishes into
//! `v-lin` (created `kind: "lineage"` if absent), and its repointable
//! origin export is `v-lin/origin`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use skynbd_agent::cp;
use skynbd_agent::nodemech::{NodeMechanics, ThinParams};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Stage `volume` as a publish-only thin stack mounted at `dir`.
    Mount {
        volume: String,
        dir: String,
        #[serde(default)]
        fstype: Option<String>,
    },
    /// Final publish cycle + teardown of the mount recorded at `dir`.
    Unmount { dir: String },
    /// List the mounts this daemon owns.
    Status,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountRow>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct MountRow {
    pub volume: String,
    pub dir: String,
}

/// The daemon's mount table: dir → volume, persisted so a restarted
/// daemon replays its stages (idempotent — a live dm stack
/// short-circuits; after a reboot the stack rebuilds from the
/// surviving pool file) and restarts every mount's publish cycle.
pub struct Mounts {
    mechanics: Arc<NodeMechanics>,
    cp: cp::Client,
    records_path: PathBuf,
    /// The mount table. Held only for map access — never across the
    /// stage/unstage mechanics — so `status` answers during a long
    /// operation ("what's mounted" is exactly the question someone
    /// asks while an unmount's final cycle rides out a partition).
    records: tokio::sync::Mutex<BTreeMap<String, String>>,
    /// Serializes mount/unmount/replay end to end. A separate lock
    /// from the table on purpose: the OPERATIONS must not overlap
    /// (shared dm names, one pool file per volume), but a reader of
    /// the table must not queue behind them.
    ops: tokio::sync::Mutex<()>,
}

impl Mounts {
    pub fn new(mechanics: Arc<NodeMechanics>, cp: cp::Client, state_dir: &std::path::Path) -> Self {
        let records_path = state_dir.join("wsmounts.json");
        let records = std::fs::read(&records_path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        Self {
            mechanics,
            cp,
            records_path,
            records: tokio::sync::Mutex::new(records),
            ops: tokio::sync::Mutex::new(()),
        }
    }

    fn persist(&self, records: &BTreeMap<String, String>) -> Result<(), String> {
        let tmp = self.records_path.with_extension("tmp");
        let data = serde_json::to_vec_pretty(records).expect("records serialize");
        std::fs::write(&tmp, data)
            .and_then(|()| std::fs::rename(&tmp, &self.records_path))
            .map_err(|e| format!("persist {}: {e}", self.records_path.display()))
    }

    /// Re-stage every recorded mount (daemon restart / host reboot).
    /// Failures warn and leave the record in place: the volume comes
    /// back on the next explicit mount or daemon restart, and dropping
    /// the record would silently orphan a surviving pool file.
    pub async fn replay(&self) {
        let _op = self.ops.lock().await;
        let records = self.records.lock().await.clone();
        for (dir, volume) in records {
            match self.stage(&volume, &dir, None).await {
                Ok(()) => tracing::info!(%volume, %dir, "mount replayed"),
                Err(e) => tracing::warn!(%volume, %dir, error = %e, "mount replay failed"),
            }
        }
    }

    /// The staging call shared by mount and replay: sizes the volume
    /// from the control plane, ensures the `<vol>-lin` lineage exists
    /// (CSI-parity naming), and stages the publish-only stack with the
    /// filesystem mounted at `dir`.
    async fn stage(&self, volume: &str, dir: &str, fstype: Option<&str>) -> Result<(), String> {
        let size = self
            .cp
            .volume_size(volume)
            .await
            .map_err(|e| format!("control plane: {e}"))?
            .ok_or_else(|| format!("no volume {volume:?} on the server"))?;
        let lineage = format!("{volume}-lin");
        self.cp
            .create_lineage_volume(&lineage, size)
            .await
            .map_err(|e| format!("lineage {lineage}: {e}"))?;
        self.mechanics
            .stage(
                volume,
                dir,
                fstype,
                false,
                Some(ThinParams {
                    size_bytes: size,
                    lineage: Some(lineage),
                    reclaim: Default::default(),
                }),
            )
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn mount(&self, volume: &str, dir: &str, fstype: Option<&str>) -> Result<(), String> {
        // The ops lock spans check → stage → record, so the short
        // table-lock windows cannot interleave with another operation's
        // — the preconditions checked here still hold at insert time.
        let _op = self.ops.lock().await;
        {
            let records = self.records.lock().await;
            if let Some(v) = records.get(dir) {
                return Err(format!("{dir} already mounts {v}; unmount it first"));
            }
            if let Some(d) = records.iter().find(|(_, v)| *v == volume).map(|(d, _)| d) {
                // One stack per volume: the state file, pool file, and
                // dm names are all volume-keyed.
                return Err(format!("{volume} is already mounted at {d}"));
            }
        }
        self.stage(volume, dir, fstype).await?;
        let mut records = self.records.lock().await;
        records.insert(dir.to_string(), volume.to_string());
        self.persist(&records)
    }

    pub async fn unmount(&self, dir: &str) -> Result<(), String> {
        let _op = self.ops.lock().await;
        let volume = self
            .records
            .lock()
            .await
            .get(dir)
            .cloned()
            .ok_or_else(|| format!("{dir} is not an agent-managed mount"))?;
        // The final publish cycle runs inside unstage (D-M13); an error
        // keeps the record so a retry resumes instead of stranding the
        // unpublished delta.
        self.mechanics
            .unstage(&volume, dir)
            .await
            .map_err(|e| e.to_string())?;
        let mut records = self.records.lock().await;
        records.remove(dir);
        self.persist(&records)
    }

    pub async fn status(&self) -> Vec<MountRow> {
        self.records
            .lock()
            .await
            .iter()
            .map(|(dir, volume)| MountRow {
                volume: volume.clone(),
                dir: dir.clone(),
            })
            .collect()
    }

    pub async fn handle(&self, req: Request) -> Reply {
        let result = match req {
            Request::Mount {
                volume,
                dir,
                fstype,
            } => self.mount(&volume, &dir, fstype.as_deref()).await,
            Request::Unmount { dir } => self.unmount(&dir).await,
            Request::Status => {
                return Reply {
                    ok: true,
                    error: None,
                    mounts: self.status().await,
                }
            }
        };
        match result {
            Ok(()) => Reply {
                ok: true,
                ..Default::default()
            },
            Err(e) => Reply {
                ok: false,
                error: Some(e),
                ..Default::default()
            },
        }
    }
}

/// Bind the control socket (replacing a stale one) with a 0700 parent
/// directory — the file mode IS the authorization, as on the CSI
/// driver's node-local NBD socket.
pub fn bind(socket: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
    if let Some(parent) = socket.parent() {
        use std::os::unix::fs::PermissionsExt;
        let own = |p: &std::path::Path| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
        };
        // 0700 ONLY on a directory this call created. `create_dir_all`
        // succeeds on an existing directory, and tightening a
        // pre-existing, config-supplied parent would chmod whatever the
        // config points at — `socket = "/run/ctl.sock"` is a perfectly
        // natural value, and `chmod 0700 /run` breaks every non-root
        // consumer on the host. A parent that already exists belongs to
        // the operator; its mode is their statement, not ours.
        match std::fs::create_dir(parent) {
            Ok(()) => own(parent)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            // Deeper levels missing: create them all, then claim only
            // the leaf — everything above it existed or was equally
            // just created for us alone.
            Err(_) => {
                std::fs::create_dir_all(parent)?;
                own(parent)?;
            }
        }
    }
    let _ = std::fs::remove_file(socket);
    tokio::net::UnixListener::bind(socket)
}

/// Accept loop: one request line, one reply line, close. Mount and
/// unmount serialize on the operations lock for their whole
/// stage/unstage — correct (the mechanics serialize internally
/// anyway) and simple — while `status` reads the table without
/// queueing behind them.
pub async fn serve(listener: tokio::net::UnixListener, mounts: Arc<Mounts>) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "control socket accept failed");
                continue;
            }
        };
        let mounts = mounts.clone();
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut line = String::new();
            if BufReader::new(r).read_line(&mut line).await.is_err() {
                return;
            }
            let reply = match serde_json::from_str::<Request>(line.trim()) {
                Ok(req) => mounts.handle(req).await,
                Err(e) => Reply {
                    ok: false,
                    error: Some(format!("bad request: {e}")),
                    ..Default::default()
                },
            };
            let mut out = serde_json::to_vec(&reply).expect("reply serializes");
            out.push(b'\n');
            let _ = w.write_all(&out).await;
        });
    }
}

/// Client side, used by `mount --thin` / `unmount`: one round trip.
pub async fn call(socket: &std::path::Path, req: &Request) -> Result<Reply, String> {
    let stream = tokio::net::UnixStream::connect(socket).await.map_err(|e| {
        format!(
            "cannot reach the agent control socket {} ({e}); is the \
             skynbd-agent service running with a [mounts] section?",
            socket.display()
        )
    })?;
    let (r, mut w) = stream.into_split();
    let mut line = serde_json::to_vec(req).expect("request serializes");
    line.push(b'\n');
    w.write_all(&line).await.map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(r)
        .read_line(&mut reply)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(reply.trim()).map_err(|e| format!("bad reply: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynbd_agent::hostcmd::mock::MockRunner;
    use skynbd_agent::nodemech::{NbdEndpoint, NodeConfig};
    use skynbd_agent::provision::ThinStackSpec;
    use skynbd_agent::thinstack::ThinStack;

    /// Inert stack: nothing in these tests reaches a real build (the
    /// control plane is unreachable on purpose), but the mechanics
    /// constructor wants one for thin support to exist at all.
    struct NoStack;
    impl ThinStack for NoStack {
        fn build(&self, _: &ThinStackSpec) -> Result<(), String> {
            Ok(())
        }
        fn built(&self, _: &str) -> Result<bool, String> {
            Ok(false)
        }
        fn teardown(&self, _: &ThinStackSpec) -> Result<(), String> {
            Ok(())
        }
        fn assert_guest_discard_mask(&self, _: &ThinStackSpec) -> Result<(), String> {
            Ok(())
        }
    }

    fn mounts(dir: &std::path::Path) -> Arc<Mounts> {
        let sysfs = dir.join("sys-block");
        std::fs::create_dir_all(sysfs.join("nbd0")).unwrap();
        let mechanics = Arc::new(NodeMechanics::new(
            NodeConfig {
                endpoint: NbdEndpoint::Tcp {
                    host: "skynbd.example".into(),
                    port: 10809,
                    tls_hostname: None,
                    creds_dir: None,
                },
                sysfs_block: sysfs,
                state_dir: dir.join("state"),
                default_fstype: "ext4".into(),
                cache_dir: Some(dir.join("cache")),
                cache_bytes: 64 << 20,
                chunk_bytes: 64 << 10,
                read_cache_dir: None,
                read_cache_bytes: 0,
            },
            Arc::new(MockRunner::default()),
            Some(Arc::new(NoStack) as Arc<dyn ThinStack>),
            None,
        ));
        // 127.0.0.1:1 — connection refused, deterministically: these
        // tests exercise the socket protocol and the mount table, not
        // a staged volume.
        let client = cp::Client::new("http://127.0.0.1:1", None);
        Arc::new(Mounts::new(mechanics, client, dir))
    }

    #[tokio::test]
    async fn protocol_round_trips_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let m = mounts(dir.path());
        let sock = dir.path().join("ctl.sock");
        let listener = bind(&sock).unwrap();
        tokio::spawn(serve(listener, m));

        // Status on a fresh daemon: ok, no mounts.
        let r = call(&sock, &Request::Status).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert!(r.mounts.is_empty());

        // A mount whose control plane is down fails with a reply that
        // says so — over the wire, not as a dropped connection.
        let r = call(
            &sock,
            &Request::Mount {
                volume: "v".into(),
                dir: "/mnt/v".into(),
                fstype: None,
            },
        )
        .await
        .unwrap();
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap().contains("control plane"),
            "{r:?}"
        );

        // Garbage is a reply too.
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let s = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let (rd, mut wr) = s.into_split();
        wr.write_all(b"not json\n").await.unwrap();
        let mut line = String::new();
        BufReader::new(rd).read_line(&mut line).await.unwrap();
        let reply: Reply = serde_json::from_str(line.trim()).unwrap();
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("bad request"));
    }

    /// The 0700 claim lands only on a parent `bind` itself created. A
    /// pre-existing, config-supplied parent keeps the operator's mode —
    /// `socket = "/run/ctl.sock"` must never become `chmod 0700 /run`.
    #[tokio::test]
    async fn bind_does_not_tighten_a_pre_existing_parent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("run");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _l = super::bind(&parent.join("ctl.sock")).unwrap();
        let mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "a pre-existing parent keeps the operator's mode"
        );

        // A parent this bind creates is ours: locked to 0700, because
        // the file mode IS the authorization.
        let fresh = dir.path().join("state/deep");
        let _l2 = super::bind(&fresh.join("ctl.sock")).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a created parent is claimed and locked down");
    }

    #[tokio::test]
    async fn unmount_of_an_unrecorded_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let m = mounts(dir.path());
        let err = m.unmount("/mnt/ghost").await.unwrap_err();
        assert!(err.contains("not an agent-managed mount"), "{err}");
    }

    /// The records file survives a daemon restart: a new `Mounts` over
    /// the same state dir sees the same table (replay then re-stages
    /// from it; here we only assert the table itself).
    #[tokio::test]
    async fn records_reload_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("wsmounts.json"), br#"{"/mnt/v":"v"}"#).unwrap();
        let m = mounts(dir.path());
        let rows = m.status().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].volume.as_str(), rows[0].dir.as_str()),
            ("v", "/mnt/v")
        );
    }
}
