"""Reproducer for #80: the control program's answer to a flush whose
start sector is the kernel's (sector_t)-1. Adds one test to a scratch
checkout, runs it, applies a one-line fix, runs it again, restores."""
import subprocess, sys, pathlib, os
root = pathlib.Path(sys.argv[1]); target = sys.argv[2]
f = root / "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
orig = f.read_text()
anchor = "        fn read(tag: u16, at: u64) -> Fetched {\n"
test = '''        /// A flush as the kernel hands it: the block layer's flush
        /// request has no sector, (sector_t)-1. It is answered with
        /// the file's sync and counted.
        #[test]
        fn review_a_flush_with_no_sector_is_answered() {
            let flush = Fetched {
                tag: 2,
                desc: Desc {
                    op_flags: u32::from(op::FLUSH),
                    nr_sectors: 0,
                    start_sector: u64::MAX,
                },
            };
            let (queue, _lines, counts) =
                run(vec![(vec![flush], Vec::new(), Ok(Woken::Work))], Plan::default());
            assert_eq!(queue.commits, vec![(2, 0, false)]);
            assert_eq!(counts.flushes.load(Ordering::Relaxed), 1);
        }

'''
assert orig.count(anchor) == 1
fix_a = "        let at = f.desc.start_sector * 512;\n"
fix_b = "        let at = f.desc.start_sector.saturating_mul(512);\n"
assert orig.count(fix_a) == 1
def run(label):
    p = subprocess.run(["cargo", "test", "--locked", "-q", "-p", "skyzfs-ublk-sys", "--features", "control-test-program",
                        "--bin", "skyzfs-ublk-control", "review_"], cwd=root, capture_output=True, text=True,
                       env=dict(os.environ, CARGO_TARGET_DIR=target), timeout=1500)
    out = p.stdout + p.stderr
    keep = [l for l in out.splitlines() if "test result" in l or "panicked" in l or "overflow" in l or l.startswith("test ")]
    print(label, "rc", p.returncode); print("\n".join("  " + l for l in keep))
try:
    f.write_text(orig.replace(anchor, test + anchor))
    run("as at f3f7d77:")
    f.write_text(orig.replace(anchor, test + anchor).replace(fix_a, fix_b))
    run("with saturating_mul:")
finally:
    f.write_text(orig)
