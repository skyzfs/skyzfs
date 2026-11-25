//! Next look at #147, scratch tests of the client's L5 checks (planted
//! in a worktree at the head as `crates/skyzfs-control/src/nl147_client.rs`
//! with `#[cfg(test)] mod nl147_client;` in `lib.rs`, then removed). Run
//! as root here; c2 and c4 need another user (uid 54321).
//!
//! c1: a directory, a FIFO and a regular file at the path: refused before
//!     any connect, `PermissionDenied`.
//! c2: the race between the `lstat` and the connect: a thread swaps the
//!     path between a root-owned socket nobody answers on and a symbolic
//!     link to another user's listener. Asks that pass the `lstat` and
//!     then reach the other user's listener are refused by the peer check
//!     (`answered by user`); none is accepted.
//! c3: the same race with the link to another root listener: the asks
//!     that pass the `lstat` and reach it are accepted (what the peer
//!     check cannot tell apart).
//! c4: a listener made by root that then runs as another user (listen,
//!     then setuid): the peer check reads the credentials of the listen,
//!     and the ask is accepted.

use std::io;
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::client::connect_counted;

fn in_ms(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

fn root() -> bool {
    rustix::process::geteuid().as_raw() == 0
}

#[test]
fn c1_a_directory_a_fifo_or_a_file_at_the_path_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let d = dir.path().join("d");
    std::fs::create_dir(&d).expect("dir");
    let f = dir.path().join("f");
    std::fs::write(&f, b"").expect("file");
    let fifo = dir.path().join("p");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    for path in [&d, &f, &fifo] {
        if path == &fifo && !made.as_ref().is_ok_and(|s| s.success()) {
            continue;
        }
        let mut tries = 0;
        let e = connect_counted(path, in_ms(500), &mut tries).expect_err("not a socket");
        eprintln!("c1 {}: {:?} {e} (tries {tries})", path.display(), e.kind());
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(tries, 0);
    }
}

/// A Python listener as `uid` at `path`, answering nothing.
fn listener_as(uid: u32, path: &std::path::Path, setuid_after_listen: bool) -> std::process::Child {
    use std::io::BufRead as _;
    use std::os::unix::process::CommandExt as _;
    let code = if setuid_after_listen {
        format!(
            "import socket, sys, time, os\n\
             s = socket.socket(socket.AF_UNIX)\n\
             s.bind(sys.argv[1]); s.listen(64)\n\
             os.setgid({uid}); os.setuid({uid})\n\
             print('ready', os.getuid(), flush=True)\n\
             while True:\n    c, _ = s.accept()\n"
        )
    } else {
        "import socket, sys, time, os\n\
         s = socket.socket(socket.AF_UNIX)\n\
         s.bind(sys.argv[1]); s.listen(64)\n\
         print('ready', os.getuid(), flush=True)\n\
         while True:\n    c, _ = s.accept()\n"
            .to_string()
    };
    let mut cmd = std::process::Command::new("python3");
    cmd.args(["-c", &code, path.to_str().expect("UTF-8")])
        .stdout(std::process::Stdio::piped());
    if !setuid_after_listen {
        cmd.uid(uid);
    }
    let mut child = cmd.spawn().expect("python3");
    let out = child.stdout.take().expect("stdout");
    let mut line = String::new();
    io::BufReader::new(out).read_line(&mut line).expect("ready");
    assert!(line.starts_with("ready"), "{line}");
    eprintln!("listener: {}", line.trim());
    child
}

fn race(target_uid: u32) -> (usize, usize, usize, usize) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("dir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).expect("open");
    // The daemon's own socket: root's, answered by a listener here.
    let real = dir.path().join("real.sock");
    let real_listener = UnixListener::bind(&real).expect("bound");
    real_listener.set_nonblocking(true).expect("non-blocking");
    let drain_stop = Arc::new(AtomicBool::new(false));
    let drainer = {
        let stop = Arc::clone(&drain_stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if real_listener.accept().is_err() {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
    };
    // The other listener, of `target_uid`.
    let other = dir.path().join("other.sock");
    let mut child = listener_as(target_uid, &other, false);
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&other, &link).expect("link");
    let real_copy = dir.path().join("real2");
    std::fs::hard_link(&real, &real_copy).expect("a second name of the root socket");
    let path = dir.path().join("control");
    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        let (path, link, real_copy) = (path.clone(), link.clone(), real_copy.clone());
        let tmp = dir.path().join("tmp");
        std::thread::spawn(move || {
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let from = if n % 2 == 0 { &real_copy } else { &link };
                // A new name for one of the two, renamed over the path.
                let _ = std::fs::remove_file(&tmp);
                if std::fs::hard_link(from, &tmp).is_err() {
                    // The link itself: a fresh symbolic link.
                    let _ = std::os::unix::fs::symlink(std::fs::read_link(from).unwrap_or_default(), &tmp);
                }
                let _ = std::fs::rename(&tmp, &path);
                n += 1;
            }
        })
    };
    let (mut ok_other, mut ok_real, mut refused_peer, mut refused_lstat) = (0, 0, 0, 0);
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(3) {
        match connect_counted(&path, in_ms(200), &mut 0) {
            Ok(stream) => {
                let peer = rustix::net::sockopt::socket_peercred(&stream)
                    .map(|c| c.uid.as_raw())
                    .unwrap_or(u32::MAX);
                // Which listener: the peer's pid.
                let pid = rustix::net::sockopt::socket_peercred(&stream)
                    .map(|c| c.pid.as_raw_nonzero().get())
                    .unwrap_or(0);
                if pid == std::process::id() as i32 {
                    ok_real += 1;
                } else {
                    ok_other += 1;
                    let _ = peer;
                }
            }
            Err(e) if e.to_string().contains("answered by user") => refused_peer += 1,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => refused_lstat += 1,
            Err(_) => {}
        }
    }
    stop.store(true, Ordering::Relaxed);
    let _ = swapper.join();
    drain_stop.store(true, Ordering::Relaxed);
    let _ = drainer.join();
    let _ = child.kill();
    let _ = child.wait();
    (ok_real, ok_other, refused_peer, refused_lstat)
}

#[test]
fn c2_the_race_to_another_users_listener_is_refused_by_the_peer_check() {
    if !root() {
        eprintln!("c2: not root, skipped");
        return;
    }
    let (ok_real, ok_other, refused_peer, refused_lstat) = race(54_321);
    eprintln!(
        "c2: to root's socket {ok_real}; to the other user's {ok_other}; refused by the peer check {refused_peer}; by the lstat {refused_lstat}"
    );
    assert_eq!(ok_other, 0, "an ask reached another user's listener");
}

#[test]
fn c3_the_race_to_another_root_listener_is_accepted() {
    if !root() {
        eprintln!("c3: not root, skipped");
        return;
    }
    let (ok_real, ok_other, refused_peer, refused_lstat) = race(0);
    eprintln!(
        "c3: to the daemon's socket {ok_real}; to another root listener through the link {ok_other}; refused by the peer check {refused_peer}; by the lstat {refused_lstat}"
    );
}

#[test]
fn c4_a_listener_that_drops_to_another_user_after_listen_is_root_to_the_peer_check() {
    if !root() {
        eprintln!("c4: not root, skipped");
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("drop.sock");
    let mut child = listener_as(54_321, &path, true);
    let r = connect_counted(&path, in_ms(1000), &mut 0);
    eprintln!("c4: listen as root, then uid 54321: {:?}", r.as_ref().map(|_| "accepted").map_err(|e| e.to_string()));
    let _ = child.kill();
    let _ = child.wait();
}
