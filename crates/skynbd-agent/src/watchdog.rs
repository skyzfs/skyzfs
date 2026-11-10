//! Freeze watchdog (V3.2 §7.11, §9.3; agent design doc §6).
//!
//! A dedicated OS thread armed *before* the freeze, holding its own thaw
//! action. While armed it allocates nothing, logs nothing to disk, and
//! shares no locks with the cycle engine. On deadline expiry it fires the
//! thaw exactly once and records that it did, so the capture path can
//! fail the cycle cleanly instead of publishing a suspect snapshot.
//!
//! The thaw action is injected (`FnMut`), which is what makes the whole
//! arm/disarm/fire protocol testable without root: production injects an
//! `FITHAW` ioctl on a pre-opened fd; tests inject a counter.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const FIRED: u8 = 2;

pub struct Watchdog {
    tx: mpsc::Sender<Cmd>,
    state: Arc<AtomicU8>,
    handle: Option<std::thread::JoinHandle<()>>,
}

enum Cmd {
    Arm(Duration),
    Disarm,
    Quit,
}

/// Outcome of a freeze window, observed at disarm time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeOutcome {
    /// Disarmed in time; the capture owns the thaw.
    Clean,
    /// The watchdog force-thawed: the capture MUST fail (delete `S_n`).
    ForceThawed,
}

impl Watchdog {
    /// `thaw` runs on the watchdog thread when a deadline expires.
    pub fn spawn(mut thaw: impl FnMut() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let state = Arc::new(AtomicU8::new(IDLE));
        let st = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("freeze-watchdog".into())
            .spawn(move || loop {
                match rx.recv() {
                    Ok(Cmd::Arm(budget)) => {
                        st.store(ARMED, Ordering::SeqCst);
                        // Wait for disarm, or fire at the deadline.
                        match rx.recv_timeout(budget) {
                            Ok(Cmd::Disarm) => {
                                st.store(IDLE, Ordering::SeqCst);
                            }
                            Ok(Cmd::Quit) => return,
                            Ok(Cmd::Arm(_)) => {
                                // Protocol violation; safest is to fire.
                                thaw();
                                st.store(FIRED, Ordering::SeqCst);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                thaw();
                                st.store(FIRED, Ordering::SeqCst);
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    Ok(Cmd::Disarm) => { /* stale; ignore */ }
                    Ok(Cmd::Quit) | Err(_) => return,
                }
            })
            .expect("spawn watchdog thread");
        Self {
            tx,
            state,
            handle: Some(handle),
        }
    }

    /// Arm for one freeze window. Must be called *before* `FIFREEZE`.
    pub fn arm(&self, budget: Duration) {
        self.tx.send(Cmd::Arm(budget)).expect("watchdog alive");
    }

    /// Whether the armed window's deadline already fired (the thaw ran).
    /// Cheap peek for multi-operation freeze windows (the D-M15 reclaim):
    /// the caller stops issuing device ops the moment the fs may be
    /// writable again, instead of learning at disarm time.
    pub fn fired(&self) -> bool {
        self.state.load(Ordering::SeqCst) == FIRED
    }

    /// End the window; reports whether the watchdog beat us to the thaw.
    /// (`FIRED` is sticky until the next arm, so a late disarm still sees
    /// it.)
    pub fn disarm(&self) -> FreezeOutcome {
        let fired = self.state.load(Ordering::SeqCst) == FIRED;
        // Send disarm regardless; the thread ignores it when stale.
        let _ = self.tx.send(Cmd::Disarm);
        if fired {
            self.state.store(IDLE, Ordering::SeqCst);
            FreezeOutcome::ForceThawed
        } else {
            FreezeOutcome::Clean
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn disarm_in_time_is_clean_and_never_thaws() {
        let thaws = Arc::new(AtomicU32::new(0));
        let t = Arc::clone(&thaws);
        let wd = Watchdog::spawn(move || {
            t.fetch_add(1, Ordering::SeqCst);
        });
        for _ in 0..5 {
            wd.arm(Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(wd.disarm(), FreezeOutcome::Clean);
        }
        assert_eq!(thaws.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn expiry_fires_exactly_once_and_is_reported() {
        let thaws = Arc::new(AtomicU32::new(0));
        let t = Arc::clone(&thaws);
        let wd = Watchdog::spawn(move || {
            t.fetch_add(1, Ordering::SeqCst);
        });
        wd.arm(Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(wd.disarm(), FreezeOutcome::ForceThawed);
        assert_eq!(thaws.load(Ordering::SeqCst), 1);

        // Re-arm works after a fire, and stays clean.
        wd.arm(Duration::from_secs(5));
        assert_eq!(wd.disarm(), FreezeOutcome::Clean);
        assert_eq!(thaws.load(Ordering::SeqCst), 1);
    }
}
