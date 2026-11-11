// Is the memory of a freed 1 MiB buffer still mapped (so that a write to its
// old address succeeds)? Allocated on a second thread, as the queue's buffers are.
use std::io::Read;
fn mapped(addr: usize) -> bool {
    let mut maps = String::new();
    std::fs::File::open("/proc/self/maps").unwrap().read_to_string(&mut maps).unwrap();
    maps.lines().any(|l| {
        let (a, b) = l.split(' ').next().unwrap().split_once('-').unwrap();
        let (a, b) = (usize::from_str_radix(a, 16).unwrap(), usize::from_str_radix(b, 16).unwrap());
        a <= addr && addr < b
    })
}
fn round(label: &str, warm: bool) {
    let label = label.to_string();
    std::thread::spawn(move || {
        if warm {
            // What a recovery does before the queue exists: a large buffer made and freed.
            drop(vec![0u8; (1 << 20) + 4095]);
        }
        let bufs: Vec<Vec<u8>> = (0..8).map(|_| vec![0u8; (1 << 20) + 4095]).collect();
        let addrs: Vec<usize> = bufs.iter().map(|b| b.as_ptr() as usize + 4096).collect();
        drop(bufs);
        let still = addrs.iter().filter(|a| mapped(**a)).count();
        println!("{label}: {still} of {} freed 1 MiB buffers are still mapped", addrs.len());
    }).join().unwrap();
}
fn main() {
    round("no large free before", false);
    round("one large buffer freed before", true);
}
