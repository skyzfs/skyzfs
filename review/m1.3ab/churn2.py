import sys
p = sys.argv[1]
s = open(p).read()
probe = '''
    #[test]
    #[ignore]
    fn probe_churn_at_new_layout() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let stop = Arc::new(AtomicBool::new(false));
        let churn: Vec<_> = (0..3)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let t = std::thread::spawn(|| std::hint::black_box(vec![0u8; 4096]).len());
                        let _ = t.join();
                    }
                })
            })
            .collect();
        let secs: u64 = std::env::var("PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let (mut n, mut none, mut own_false, mut reached, mut missed, mut run, mut longest) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        while std::time::Instant::now() < end {
            n += 1;
            match with_a_hole() {
                None => { none += 1; run += 1; }
                Some((own, larger)) => {
                    if !own { own_false += 1; }
                    if larger { missed += 1; run += 1; } else { reached += 1; run = 0; }
                }
            }
            longest = longest.max(run);
        }
        stop.store(true, Ordering::Relaxed);
        for c in churn { let _ = c.join(); }
        println!("attempts {n}; not counted (hole read as mapped) {none}; fourth page read as unmapped {own_false}; counted and reached {reached}; counted and missed {missed}; longest run without a pass {longest}");
    }
'''
marker = "    /// Map, make the hole, and ask `mapped_in` about the fourth page"
assert marker in s
s = s.replace(marker, probe + "\n" + marker, 1)
open(p, 'w').write(s)
