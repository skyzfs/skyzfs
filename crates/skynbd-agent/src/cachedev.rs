//! Cache backing-device preparation for the thin stack (V3.2 §6.5, CSI
//! design 6h): a per-volume **fallocate-preallocated** file attached as
//! a loop device with direct IO where the filesystem supports it.
//! Shared by the CSI node plugin and the workstation CLI (`--thin`) so
//! there is exactly one implementation of these rules:
//!
//! - **Preallocated, never sparse**: a sparse backing file can hit
//!   ENOSPC *inside the writeback path* — dirty dm-cache blocks that
//!   cannot land can wedge the whole stack. Reservation turns
//!   out-of-space into a clean prepare-time failure, and unwritten
//!   extents still read as zeros, which is the fresh-metadata contract
//!   dm-cache and dm-thin require.
//! - A failed preallocation removes the half-made file so no later
//!   prepare silently adopts a sparse one.
//! - A **surviving** file is reused untouched — it carries the dm-cache
//!   and thin-pool metadata across restages and reboots (dm reads its
//!   own metadata back). Deciding whether a *missing* file is "fresh
//!   volume" or "V3.2 §15.7 total local loss" needs the caller's
//!   binding state and stays with the caller.
//! - **Direct IO** on the loop avoids double page-caching under the dm
//!   stack; filesystems without O_DIRECT fall back to buffered (the
//!   result says which, so callers can log it).

use std::path::Path;

use crate::hostcmd::Runner;

#[derive(Debug, thiserror::Error)]
pub enum CacheDevError {
    #[error("{cmd} failed: {detail}")]
    Command { cmd: String, detail: String },
    #[error("io {0}: {1}")]
    Io(std::path::PathBuf, std::io::Error),
}

/// An attached cache backing device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDev {
    /// `/dev/loopN`
    pub loop_dev: String,
    /// Whether the loop got O_DIRECT (false = buffered fallback).
    pub direct_io: bool,
}

async fn run_ok<R: Runner + ?Sized>(
    runner: &R,
    cmd: &str,
    args: Vec<String>,
) -> Result<crate::hostcmd::Output, CacheDevError> {
    let out = runner
        .run(cmd, &args)
        .await
        .map_err(|e| CacheDevError::Command {
            cmd: cmd.into(),
            detail: e.to_string(),
        })?;
    if out.success {
        Ok(out)
    } else {
        Err(CacheDevError::Command {
            cmd: format!("{cmd} {}", args.join(" ")),
            detail: format!("{}{}", out.stdout, out.stderr),
        })
    }
}

/// The loop device already backed by `file`, if there is one, and
/// whether it carries O_DIRECT. `None` also covers a `losetup` that
/// cannot answer the question: the attach that follows is what decides,
/// exactly as before this probe existed.
///
/// Public for the unstage path: a §7 two-pool swing replaces the pool
/// file's loop mid-life (the B slot's loop survives the canonicalize),
/// so the loop device recorded at stage time can be stale — the file is
/// the durable identity, the loop is not. `losetup --associated`
/// matches by the file's dev/inode, so it follows the file across the
/// swing's rename where a recorded device name cannot.
pub async fn existing_association<R: Runner + ?Sized>(runner: &R, file: &Path) -> Option<CacheDev> {
    let out = runner
        .run(
            "losetup",
            &[
                "--noheadings".into(),
                "--output".into(),
                "NAME,DIO".into(),
                "--associated".into(),
                file.display().to_string(),
            ],
        )
        .await
        .ok()?;
    if !out.success {
        return None;
    }
    let line = out.stdout.lines().find(|l| !l.trim().is_empty())?;
    let mut cols = line.split_whitespace();
    let loop_dev = cols.next()?.to_string();
    Some(CacheDev {
        loop_dev,
        direct_io: cols.next() == Some("1"),
    })
}

/// Ensure `file` exists preallocated at `bytes`, then loop-attach it —
/// **adopting** an association the file already has rather than adding
/// another.
///
/// `losetup --find` allocates a *new* device every time, so without the
/// adopt this is not idempotent in the way its callers assume: a stage
/// that retries (kubelet retries a failed NodeStageVolume forever)
/// burns one `/dev/loopN` per attempt against the same file — the walk
/// to exhaustion the stage's `/dev/nbdN` rollback exists to prevent, one
/// layer down — and two loop queues over one backing file are two page
/// cache views of the same pool metadata, which is worse than the leak.
pub async fn prepare<R: Runner + ?Sized>(
    runner: &R,
    file: &Path,
    bytes: u64,
) -> Result<CacheDev, CacheDevError> {
    let surviving = file.exists();
    if !surviving {
        let f = std::fs::File::create(file).map_err(|e| CacheDevError::Io(file.into(), e))?;
        f.set_len(bytes)
            .map_err(|e| CacheDevError::Io(file.into(), e))?;
        if let Err(e) = run_ok(
            runner,
            "fallocate",
            vec!["-l".into(), bytes.to_string(), file.display().to_string()],
        )
        .await
        {
            let _ = std::fs::remove_file(file);
            return Err(e);
        }
    }
    // Only a file that outlived a previous prepare can already be
    // attached; one this call just created cannot, so the fresh path
    // stays at a single losetup.
    if surviving {
        if let Some(dev) = existing_association(runner, file).await {
            return Ok(dev);
        }
    }
    let dio = runner
        .run(
            "losetup",
            &[
                "--find".into(),
                "--show".into(),
                "--direct-io=on".into(),
                file.display().to_string(),
            ],
        )
        .await
        .map_err(|e| CacheDevError::Command {
            cmd: "losetup".into(),
            detail: e.to_string(),
        })?;
    if dio.success {
        return Ok(CacheDev {
            loop_dev: dio.stdout.trim().to_string(),
            direct_io: true,
        });
    }
    let buffered = run_ok(
        runner,
        "losetup",
        vec!["--find".into(), "--show".into(), file.display().to_string()],
    )
    .await?;
    Ok(CacheDev {
        loop_dev: buffered.stdout.trim().to_string(),
        direct_io: false,
    })
}

/// Detach a prepared loop device. The backing file stays — it is the
/// volume's durable node-local state. Callers roll back with this: a
/// stage that attached the loop and then failed must hand it back, or
/// [`prepare`]'s adopt is the only thing standing between a retry loop
/// and a node out of loop devices.
pub async fn detach<R: Runner + ?Sized>(runner: &R, loop_dev: &str) -> Result<(), CacheDevError> {
    run_ok(runner, "losetup", vec!["-d".into(), loop_dev.to_string()]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostcmd::Output;
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
    impl Runner for MockRunner {
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

    #[tokio::test]
    async fn fresh_file_is_preallocated_and_looped_direct() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        let r = MockRunner::default();
        r.push_reply(true, ""); // fallocate
        r.push_reply(true, "/dev/loop3\n"); // losetup --direct-io=on

        let dev = prepare(&r, &file, 64 << 20).await.unwrap();
        assert_eq!(dev.loop_dev, "/dev/loop3");
        assert!(dev.direct_io);
        assert!(file.exists());
        assert_eq!(file.metadata().unwrap().len(), 64 << 20);
        let calls = r.calls();
        assert_eq!(
            calls[0],
            vec![
                "fallocate".to_string(),
                "-l".into(),
                (64u64 << 20).to_string(),
                file.display().to_string()
            ]
        );
        assert!(calls[1].contains(&"--direct-io=on".to_string()));
    }

    #[tokio::test]
    async fn surviving_file_skips_preallocation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        std::fs::write(&file, b"metadata").unwrap();
        let r = MockRunner::default();
        r.push_reply(true, ""); // losetup -j: not attached
        r.push_reply(true, "/dev/loop4\n"); // losetup --find

        let dev = prepare(&r, &file, 64 << 20).await.unwrap();
        assert_eq!(dev.loop_dev, "/dev/loop4");
        assert!(!r.calls().iter().any(|c| c[0] == "fallocate"));
        // The surviving file is untouched.
        assert_eq!(std::fs::read(&file).unwrap(), b"metadata");
    }

    /// A file that is already attached is ADOPTED. `losetup --find`
    /// hands out a fresh device on every call, so without this a
    /// retrying stage attaches one more `/dev/loopN` per attempt to the
    /// same backing file — and leaves two loop queues, two page-cache
    /// views, over one pool's metadata.
    #[tokio::test]
    async fn an_existing_association_is_adopted_not_doubled() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        std::fs::write(&file, b"metadata").unwrap();
        let r = MockRunner::default();
        r.push_reply(true, "/dev/loop2 1\n"); // losetup -j: already attached, DIO on

        let dev = prepare(&r, &file, 64 << 20).await.unwrap();
        assert_eq!(dev.loop_dev, "/dev/loop2");
        assert!(dev.direct_io, "the adopted device's own DIO state");
        let calls = r.calls();
        assert_eq!(calls.len(), 1, "no second attach: {calls:?}");
        assert!(calls[0].contains(&"--associated".to_string()), "{calls:?}");
    }

    /// A `losetup` that cannot answer the association question is not
    /// an error: the attach decides, exactly as it did before the probe
    /// existed.
    #[tokio::test]
    async fn an_unanswerable_probe_falls_through_to_the_attach() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        std::fs::write(&file, b"metadata").unwrap();
        let r = MockRunner::default();
        r.push_reply(false, "unknown column: DIO"); // losetup -j
        r.push_reply(true, "/dev/loop8\n"); // losetup --find

        let dev = prepare(&r, &file, 64 << 20).await.unwrap();
        assert_eq!(dev.loop_dev, "/dev/loop8");
    }

    #[tokio::test]
    async fn direct_io_falls_back_to_buffered() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        let r = MockRunner::default();
        r.push_reply(true, ""); // fallocate
        r.push_reply(false, ""); // losetup --direct-io=on: unsupported
        r.push_reply(true, "/dev/loop9\n"); // losetup buffered

        let dev = prepare(&r, &file, 64 << 20).await.unwrap();
        assert_eq!(dev.loop_dev, "/dev/loop9");
        assert!(!dev.direct_io);
    }

    #[tokio::test]
    async fn failed_preallocation_removes_the_half_made_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("v.cache");
        let r = MockRunner::default();
        r.push_reply(false, ""); // fallocate: ENOSPC / unsupported

        let err = prepare(&r, &file, 64 << 20).await.unwrap_err();
        assert!(matches!(err, CacheDevError::Command { .. }), "{err}");
        // No sparse leftover for a later prepare to silently adopt.
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn detach_releases_the_loop_and_keeps_the_file() {
        let r = MockRunner::default();
        detach(&r, "/dev/loop3").await.unwrap();
        assert_eq!(r.calls()[0], vec!["losetup", "-d", "/dev/loop3"]);
    }
}
