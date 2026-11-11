//! Where a start's time goes: the lifecycle's volume and control
//! interface, each wrapped so that the steps are timed as they happen.
//! `serve` prints the result in one line when the device serves.

use std::io;
use std::time::{Duration, Instant};

use skyzfs_ublk::lifecycle::{
    AddError, Control, DeviceInfo, DeviceState, LockError, MustReach, OpenError, Shape, Volume,
};

/// A volume whose lock and open are timed.
pub struct TimedVolume<V> {
    inner: V,
    began: Instant,
    /// From the start until the lock was held: the tries and the
    /// pauses between them.
    pub lock: Duration,
    /// From the lock until the open began: the failure note and the
    /// acknowledged position are read in that time.
    pub position: Duration,
    /// The open: the store's recovery.
    pub recovery: Duration,
    locked: Option<Instant>,
}

impl<V> TimedVolume<V> {
    /// `inner`, timed from now.
    pub fn new(inner: V) -> Self {
        Self {
            inner,
            began: Instant::now(),
            lock: Duration::ZERO,
            position: Duration::ZERO,
            recovery: Duration::ZERO,
            locked: None,
        }
    }
}

impl<V: Volume> Volume for TimedVolume<V> {
    type Lock = V::Lock;
    type Open = V::Open;

    fn lock(&mut self) -> Result<V::Lock, LockError> {
        let lock = self.inner.lock()?;
        let now = Instant::now();
        self.lock = now - self.began;
        self.locked = Some(now);
        Ok(lock)
    }

    fn open(
        &mut self,
        lock: V::Lock,
        must_reach: Option<MustReach>,
        acknowledge_failure: bool,
    ) -> Result<(V::Open, u64), (OpenError, V::Lock)> {
        let began = Instant::now();
        if let Some(locked) = self.locked {
            self.position = began - locked;
        }
        let opened = self.inner.open(lock, must_reach, acknowledge_failure);
        self.recovery += began.elapsed();
        opened
    }
}

/// A control interface that notes how long a device stayed live while
/// it was asked about, and how long the begin of a recovery took.
pub struct TimedControl<C> {
    /// The interface itself.
    pub inner: C,
    /// When a device was first reported live, until it is reported
    /// otherwise.
    live_since: Option<Instant>,
    /// How long devices were reported live while the start asked.
    pub live: Duration,
    /// The begin of a recovery, its retries included.
    pub begin: Duration,
}

impl<C> TimedControl<C> {
    /// `inner`, with nothing noted yet.
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            live_since: None,
            live: Duration::ZERO,
            begin: Duration::ZERO,
        }
    }
}

impl<C: Control> Control for TimedControl<C> {
    fn can_reissue(&mut self) -> io::Result<bool> {
        self.inner.can_reissue()
    }
    fn info(&mut self, number: u32) -> io::Result<Option<DeviceInfo>> {
        let info = self.inner.info(number)?;
        let live = info.is_some_and(|i| i.state == DeviceState::Live);
        match (live, self.live_since) {
            (true, None) => self.live_since = Some(Instant::now()),
            (false, Some(since)) => {
                self.live += since.elapsed();
                self.live_since = None;
            }
            _ => {}
        }
        Ok(info)
    }
    fn add(&mut self, number: u32, shape: &Shape) -> Result<(), AddError> {
        self.inner.add(number, shape)
    }
    fn disown(&mut self, number: u32) {
        self.inner.disown(number);
    }
    fn delete(&mut self, number: u32) -> io::Result<()> {
        self.inner.delete(number)
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        let began = Instant::now();
        let result = self.inner.begin_recovery(number);
        self.begin += began.elapsed();
        result
    }
    fn client_holds(&mut self, number: u32) -> Option<bool> {
        self.inner.client_holds(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted(Vec<DeviceState>);

    impl Control for Scripted {
        fn can_reissue(&mut self) -> io::Result<bool> {
            Ok(true)
        }
        fn info(&mut self, _number: u32) -> io::Result<Option<DeviceInfo>> {
            let state = if self.0.len() > 1 {
                self.0.remove(0)
            } else {
                self.0[0]
            };
            Ok(Some(DeviceInfo {
                state,
                shape: Shape {
                    size: 0,
                    queues: 1,
                    depth: 1,
                    flags: 0,
                },
            }))
        }
        fn add(&mut self, _number: u32, _shape: &Shape) -> Result<(), AddError> {
            Ok(())
        }
        fn disown(&mut self, _number: u32) {}
        fn delete(&mut self, _number: u32) -> io::Result<()> {
            Ok(())
        }
        fn begin_recovery(&mut self, _number: u32) -> io::Result<()> {
            std::thread::sleep(Duration::from_millis(20));
            Ok(())
        }
        fn client_holds(&mut self, _number: u32) -> Option<bool> {
            None
        }
    }

    /// The time a device is reported live is counted from the first
    /// such answer to the first other one, and the begin of a recovery
    /// is timed.
    #[test]
    fn the_live_wait_and_the_begin_of_a_recovery_are_timed() {
        let script = vec![
            DeviceState::Waiting,
            DeviceState::Live,
            DeviceState::Live,
            DeviceState::Waiting,
        ];
        let mut control = TimedControl::new(Scripted(script));
        control.info(0).expect("info");
        assert_eq!(control.live, Duration::ZERO);
        control.info(0).expect("info");
        std::thread::sleep(Duration::from_millis(30));
        control.info(0).expect("info");
        assert_eq!(control.live, Duration::ZERO, "still live");
        control.info(0).expect("info");
        assert!(control.live >= Duration::from_millis(30));
        let noted = control.live;
        control.info(0).expect("info");
        assert_eq!(control.live, noted);
        control.begin_recovery(0).expect("begin");
        assert!(control.begin >= Duration::from_millis(20));
    }
}
