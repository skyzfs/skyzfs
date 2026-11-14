// Does a request that io_uring punts to its workers (IOSQE_ASYNC) show a
// thread iou-wrk-<tid> in /proc/self/task, and what is its wchan? And does
// the worker stay after its request is done?
use io_uring::{opcode, squeue, types, IoUring};
use std::os::fd::AsRawFd;

fn tasks() -> Vec<String> {
    let mut out = Vec::new();
    for t in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let p = t.path();
        let comm = std::fs::read_to_string(p.join("comm")).unwrap_or_default();
        let wchan = std::fs::read_to_string(p.join("wchan")).unwrap_or_default();
        out.push(format!("{}={} (wchan {})", t.file_name().to_string_lossy(), comm.trim(), wchan.trim()));
    }
    out
}

fn main() {
    let mut fds = [0; 2];
    unsafe { libc::pipe(fds.as_mut_ptr()) };
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    println!("main tid {tid}");
    let mut ring = IoUring::new(8).unwrap();
    let mut buf = [0u8; 8];
    let read = opcode::Read::new(types::Fd(fds[0]), buf.as_mut_ptr(), 8)
        .build()
        .flags(squeue::Flags::ASYNC)
        .user_data(1);
    unsafe { ring.submission().push(&read).unwrap() };
    ring.submit().unwrap();
    println!("right after submit: {:?}", tasks());
    std::thread::sleep(std::time::Duration::from_millis(200));
    println!("200 ms later: {:?}", tasks());
    unsafe { libc::write(fds[1], b"x".as_ptr().cast(), 1) };
    ring.submit_and_wait(1).unwrap();
    println!("read done: {:?}", ring.completion().next().map(|c| c.result()));
    std::thread::sleep(std::time::Duration::from_millis(7000));
    println!("7 s after: {:?}", tasks());
    let _ = ring.as_raw_fd();
}
