//! The signals a serving daemon acts on, taken synchronously: they are
//! blocked, and the main thread asks for the next one. No handler runs,
//! so nothing has to be safe to call from one.

use std::io;
use std::mem::MaybeUninit;
use std::time::Duration;

/// A signal the daemon acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// `SIGTERM` or `SIGINT`: stop in an orderly way.
    Stop,
    /// `SIGUSR1`: for the test build, which fails the log on it.
    User,
}

fn set() -> libc::sigset_t {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initialises the whole set it is given a
    // valid pointer to, and `sigaddset` changes a set that is
    // initialised; both get a pointer to `set`, which lives here. After
    // `sigemptyset` the set is initialised, so `assume_init` is sound.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGUSR1] {
            libc::sigaddset(set.as_mut_ptr(), signal);
        }
        set.assume_init()
    }
}

/// Block the daemon's signals in this thread and in every thread it
/// starts from now on: call it before any thread is started. They are
/// then delivered only through [`wait`].
pub fn block() -> io::Result<()> {
    let set = set();
    // SAFETY: `set` is an initialised signal set that lives across the
    // call, and a null pointer for the old mask is allowed.
    let result = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    Ok(())
}

/// The next of the daemon's signals, or `None` if none arrives within
/// `timeout`. The signals must be blocked ([`block`]).
pub fn wait(timeout: Duration) -> Option<Signal> {
    let set = set();
    let limit = libc::timespec {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_nsec: timeout.subsec_nanos() as libc::c_long,
    };
    // SAFETY: `set` and `limit` are initialised and live across the
    // call; a null pointer for the signal's details is allowed.
    let signal = unsafe { libc::sigtimedwait(&set, std::ptr::null_mut(), &limit) };
    match signal {
        libc::SIGTERM | libc::SIGINT => Some(Signal::Stop),
        libc::SIGUSR1 => Some(Signal::User),
        _ => None,
    }
}
