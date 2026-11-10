//! Host-command execution behind a trait, shared by every consumer that
//! shells out (the CSI node plugin, the workstation CLI): command
//! *planning* stays unit-testable without root, kernel devices, or a
//! server, and both binaries drive the exact same code paths.

#[derive(Debug, Clone)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    async fn run(&self, cmd: &str, args: &[String]) -> std::io::Result<Output>;
}

/// The real thing: tokio subprocesses, output captured for error context.
pub struct HostRunner;

#[async_trait::async_trait]
impl Runner for HostRunner {
    async fn run(&self, cmd: &str, args: &[String]) -> std::io::Result<Output> {
        let out = tokio::process::Command::new(cmd)
            .args(args)
            .output()
            .await?;
        Ok(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Scripted [`Runner`] for tests — records every invocation, replies
/// from a queue (default success when the queue is empty). Behind the
/// `testutil` feature so downstream crates' tests (the CSI driver, the
/// CLI) can drive the same mock the in-crate tests use.
#[cfg(any(test, feature = "testutil"))]
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockRunner {
        pub calls: Mutex<Vec<Vec<String>>>,
        pub replies: Mutex<std::collections::VecDeque<Output>>,
    }

    impl MockRunner {
        pub fn push_reply(&self, success: bool, stdout: &str) {
            self.replies.lock().unwrap().push_back(Output {
                success,
                stdout: stdout.into(),
                stderr: String::new(),
            });
        }

        pub fn calls(&self) -> Vec<Vec<String>> {
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
}
