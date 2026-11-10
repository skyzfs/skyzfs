//! Workstation mounts (CSI design §5): attach a SkyNBD volume's head, a
//! snapshot's RO export, or a fresh server fast clone on *this* machine
//! via nbd-client, and mount it — the same choreography the CSI node
//! plugin performs, minus Kubernetes. Every mount records a state file
//! keyed by mountpoint so `unmount <dir>` can tear down exactly what
//! `mount` built (device-binding pattern shared with the CSI node).
//!
//! This module handles RAW mounts only. `--thin` never reaches it: a
//! publish-only thin stack (origin-advance §5) needs the embedded
//! publish cycle as its durability path, so the CLI delegates thin
//! mounts to the resident agent's control socket (see [`crate::ctl`]),
//! which stages the same stack the CSI node stages. State files
//! recorded by a pre-§5 (Shape-B) thin mount are unsupported (nothing
//! is in production; previous formats are unsupported).
//!
//! Host commands run behind the shared [`Runner`] seam so the planning
//! logic is unit-testable without root, kernel NBD, or a server.

use std::path::{Path, PathBuf};

pub use skynbd_agent::hostcmd::{HostRunner, Output, Runner};

#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("no free NBD device (is the nbd module loaded?)")]
    NoFreeDevice,
    #[error("{cmd} failed: {detail}")]
    Command { cmd: String, detail: String },
    #[error("io {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0} is not a skynbd mountpoint (no state recorded)")]
    NotMounted(PathBuf),
    #[error("{0} already has a skynbd mount recorded; unmount it first")]
    AlreadyMounted(PathBuf),
}

/// What to attach (CSI design §5's three mount shapes). Clone creation
/// itself is a control-plane call the caller makes first; by the time
/// mechanics run, a clone mounts like any other head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountKind {
    /// `<volume>/head`, read-write.
    Head,
    /// `<volume>/snap/<name>`, read-only (published snapshots are served
    /// RO by the registry).
    RoSnapshot(String),
}

/// RO mount options per filesystem: crash-consistent snapshot images can
/// hold a dirty journal, and replay is impossible on a read-only device.
fn ro_mount_opts(fstype: &str) -> &'static str {
    match fstype {
        "ext2" | "ext3" | "ext4" => "ro,noload",
        "xfs" => "ro,norecovery",
        _ => "ro",
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MountState {
    device: String,
    export: String,
    volume: String,
    dir: PathBuf,
}

#[derive(Clone)]
pub struct MountCtx {
    pub nbd_host: String,
    pub nbd_port: u16,
    /// TLS creds dir (client-cert/client-key/ca-cert PEMs from
    /// enrollment); `None` = plaintext NBD.
    pub creds_dir: Option<PathBuf>,
    pub tls_hostname: Option<String>,
    /// `/sys/block` (overridable in tests).
    pub sysfs_block: PathBuf,
    /// Where mount state files live.
    pub state_dir: PathBuf,
    pub default_fstype: String,
}

pub struct Workstation<R> {
    pub ctx: MountCtx,
    pub runner: R,
}

impl<R: Runner> Workstation<R> {
    /// State file keyed by the (canonicalized-as-given) mountpoint.
    fn state_path(&self, dir: &Path) -> PathBuf {
        let flat = dir
            .display()
            .to_string()
            .trim_matches('/')
            .replace('/', "_");
        self.ctx
            .state_dir
            .join("mounts")
            .join(format!("{flat}.json"))
    }

    fn read_state(&self, dir: &Path) -> Option<MountState> {
        let raw = std::fs::read(self.state_path(dir)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// First `/dev/nbdX` with no sysfs `pid` file (nothing attached).
    fn allocate_device(&self) -> Result<String, MountError> {
        let mut idx: Vec<u64> = std::fs::read_dir(&self.ctx.sysfs_block)
            .map_err(|e| MountError::Io(self.ctx.sysfs_block.clone(), e))?
            .filter_map(|d| {
                d.ok()?
                    .file_name()
                    .to_str()?
                    .strip_prefix("nbd")?
                    .parse()
                    .ok()
            })
            .collect();
        idx.sort_unstable();
        for i in idx {
            if !self.ctx.sysfs_block.join(format!("nbd{i}/pid")).exists() {
                return Ok(format!("/dev/nbd{i}"));
            }
        }
        Err(MountError::NoFreeDevice)
    }

    async fn run_ok(&self, cmd: &str, args: Vec<String>) -> Result<Output, MountError> {
        let out = self
            .runner
            .run(cmd, &args)
            .await
            .map_err(|e| MountError::Command {
                cmd: cmd.into(),
                detail: e.to_string(),
            })?;
        if out.success {
            Ok(out)
        } else {
            Err(MountError::Command {
                cmd: format!("{cmd} {}", args.join(" ")),
                detail: format!("{}{}", out.stdout, out.stderr),
            })
        }
    }

    fn nbd_client_args(&self, export: &str, device: &str) -> Vec<String> {
        let mut args = vec![
            "-N".into(),
            export.to_string(),
            self.ctx.nbd_host.clone(),
            self.ctx.nbd_port.to_string(),
            device.to_string(),
            "-persist".into(),
        ];
        if let Some(dir) = &self.ctx.creds_dir {
            args.extend([
                "-enable-tls".into(),
                "-certfile".into(),
                dir.join("client-cert.pem").display().to_string(),
                "-keyfile".into(),
                dir.join("client-key.pem").display().to_string(),
                "-cacertfile".into(),
                dir.join("ca-cert.pem").display().to_string(),
                "-tlshostname".into(),
                self.ctx
                    .tls_hostname
                    .clone()
                    .unwrap_or_else(|| self.ctx.nbd_host.clone()),
            ]);
        }
        args
    }

    /// Attach + (RW, blank device) format + mount into `dir`. Refuses a
    /// directory that already has a recorded mount — `unmount` first.
    pub async fn mount(
        &self,
        volume: &str,
        kind: &MountKind,
        dir: &Path,
        fstype: Option<&str>,
    ) -> Result<String, MountError> {
        if self.read_state(dir).is_some() {
            return Err(MountError::AlreadyMounted(dir.to_path_buf()));
        }
        let (export, readonly) = match kind {
            MountKind::Head => (format!("{volume}/head"), false),
            MountKind::RoSnapshot(s) => (format!("{volume}/snap/{s}"), true),
        };
        let device = self.allocate_device()?;
        self.run_ok("nbd-client", self.nbd_client_args(&export, &device))
            .await?;
        let fs_dev = device.clone();

        let fstype = fstype.unwrap_or(&self.ctx.default_fstype).to_string();
        if !readonly {
            // Format only a blank device: blkid exits non-zero with no
            // output when it finds no signature.
            let probe = self
                .runner
                .run(
                    "blkid",
                    &[
                        "-o".into(),
                        "value".into(),
                        "-s".into(),
                        "TYPE".into(),
                        fs_dev.clone(),
                    ],
                )
                .await
                .map_err(|e| MountError::Command {
                    cmd: "blkid".into(),
                    detail: e.to_string(),
                })?;
            if probe.stdout.trim().is_empty() {
                self.run_ok(&format!("mkfs.{fstype}"), vec![fs_dev.clone()])
                    .await?;
            }
        }
        std::fs::create_dir_all(dir).map_err(|e| MountError::Io(dir.to_path_buf(), e))?;
        let mut mount_args = vec!["-t".into(), fstype.clone()];
        if readonly {
            mount_args.extend(["-o".into(), ro_mount_opts(&fstype).into()]);
        }
        mount_args.extend([fs_dev.clone(), dir.display().to_string()]);
        self.run_ok("mount", mount_args).await?;

        let sp = self.state_path(dir);
        std::fs::create_dir_all(sp.parent().unwrap()).map_err(|e| MountError::Io(sp.clone(), e))?;
        let state = serde_json::to_vec(&MountState {
            device: device.clone(),
            export: export.clone(),
            volume: volume.to_string(),
            dir: dir.to_path_buf(),
        })
        .expect("state serializes");
        std::fs::write(&sp, state).map_err(|e| MountError::Io(sp, e))?;
        Ok(fs_dev)
    }

    /// Tear down whatever `mount` built at `dir`: umount (tolerating
    /// already-unmounted) + detach + drop state.
    pub async fn unmount(&self, dir: &Path) -> Result<(), MountError> {
        let Some(state) = self.read_state(dir) else {
            return Err(MountError::NotMounted(dir.to_path_buf()));
        };
        let um = self
            .runner
            .run("umount", &[dir.display().to_string()])
            .await
            .map_err(|e| MountError::Command {
                cmd: "umount".into(),
                detail: e.to_string(),
            })?;
        if !um.success
            && !um.stderr.contains("not mounted")
            && !um.stderr.contains("no mount point")
        {
            return Err(MountError::Command {
                cmd: format!("umount {}", dir.display()),
                detail: um.stderr,
            });
        }
        self.run_ok("nbd-client", vec!["-d".into(), state.device.clone()])
            .await?;
        std::fs::remove_file(self.state_path(dir))
            .map_err(|e| MountError::Io(self.state_path(dir), e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockRunner {
        calls: Mutex<Vec<Vec<String>>>,
        replies: Mutex<VecDeque<Output>>,
    }

    impl MockRunner {
        fn push_reply(&self, success: bool, stdout: &str) {
            self.replies.lock().unwrap().push_back(Output {
                success,
                stdout: stdout.into(),
                stderr: String::new(),
            });
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Runner for &MockRunner {
        async fn run(&self, cmd: &str, args: &[String]) -> std::io::Result<Output> {
            let mut call = vec![cmd.to_string()];
            call.extend(args.iter().cloned());
            self.calls.lock().unwrap().push(call);
            Ok(self.replies.lock().unwrap().pop_front().unwrap_or(Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            }))
        }
    }

    fn ws<'a>(dir: &Path, runner: &'a MockRunner, tls: bool) -> Workstation<&'a MockRunner> {
        let sysfs = dir.join("sys-block");
        for i in 0..4 {
            std::fs::create_dir_all(sysfs.join(format!("nbd{i}"))).unwrap();
        }
        Workstation {
            ctx: MountCtx {
                nbd_host: "skynbd.example".into(),
                nbd_port: 10809,
                creds_dir: tls.then(|| dir.join("creds")),
                tls_hostname: None,
                sysfs_block: sysfs,
                state_dir: dir.join("state"),
                default_fstype: "ext4".into(),
            },
            runner,
        }
    }

    #[tokio::test]
    async fn head_mount_attaches_probes_formats_and_mounts() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::default();
        let w = ws(dir.path(), &runner, true);
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(false, ""); // blkid: blank
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount

        let mnt = dir.path().join("mnt/dev");
        let dev = w
            .mount("scratch", &MountKind::Head, &mnt, None)
            .await
            .unwrap();
        assert_eq!(dev, "/dev/nbd0");

        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        let nbd = calls[0].join(" ");
        assert!(nbd.starts_with(
            "nbd-client -N scratch/head skynbd.example 10809 /dev/nbd0 -persist -enable-tls"
        ));
        assert_eq!(calls[2][0], "mkfs.ext4");
        assert_eq!(calls[3][1..3], ["-t".to_string(), "ext4".to_string()]);

        // Double mount at the same dir is refused.
        let err = w
            .mount("scratch", &MountKind::Head, &mnt, None)
            .await
            .unwrap_err();
        assert!(matches!(err, MountError::AlreadyMounted(_)), "{err}");

        // Unmount tears down exactly what mount built.
        w.unmount(&mnt).await.unwrap();
        let calls = runner.calls();
        assert_eq!(calls[4][0], "umount");
        assert_eq!(calls[5], vec!["nbd-client", "-d", "/dev/nbd0"]);
        // Replay: no state → loud error, not silent success (a human is
        // driving; a typo'd path should say so).
        assert!(matches!(
            w.unmount(&mnt).await.unwrap_err(),
            MountError::NotMounted(_)
        ));
    }

    #[tokio::test]
    async fn ro_snapshot_mount_skips_format_and_mounts_ro() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::default();
        let w = ws(dir.path(), &runner, false);
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // mount

        let mnt = dir.path().join("mnt/snap");
        w.mount(
            "vol1",
            &MountKind::RoSnapshot("g7".into()),
            &mnt,
            Some("xfs"),
        )
        .await
        .unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 2); // no blkid, no mkfs
        assert!(calls[0]
            .join(" ")
            .starts_with("nbd-client -N vol1/snap/g7 "));
        assert!(!calls[0].join(" ").contains("-enable-tls"));
        assert_eq!(
            calls[1],
            vec![
                "mount".to_string(),
                "-t".into(),
                "xfs".into(),
                "-o".into(),
                "ro,norecovery".into(),
                "/dev/nbd0".into(),
                mnt.display().to_string()
            ]
        );
    }

    #[tokio::test]
    async fn formatted_head_is_not_reformatted() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::default();
        let w = ws(dir.path(), &runner, false);
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, "ext4\n"); // blkid: formatted
        runner.push_reply(true, ""); // mount
        let mnt = dir.path().join("mnt/v");
        w.mount("v", &MountKind::Head, &mnt, None).await.unwrap();
        assert!(!runner.calls().iter().any(|c| c[0].starts_with("mkfs")));
    }

    #[tokio::test]
    async fn device_allocation_skips_busy_devices() {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::default();
        let w = ws(dir.path(), &runner, false);
        for i in 0..2 {
            std::fs::write(w.ctx.sysfs_block.join(format!("nbd{i}/pid")), "1").unwrap();
        }
        let mnt = dir.path().join("mnt/v");
        let dev = w.mount("v", &MountKind::Head, &mnt, None).await.unwrap();
        assert_eq!(dev, "/dev/nbd2");
    }
}
