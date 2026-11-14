"""Plants for #81, one at a time; each file is restored after."""
import subprocess, sys, pathlib, os, re
root = pathlib.Path(sys.argv[1]); target = sys.argv[2]
ctl = "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
tr = "crates/skyzfs-ublk/src/translate.rs"
fix = """        let at = || {
            f.desc
                .start_sector
                .checked_mul(512)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))
        };
"""
flush_ret = "        if op == op::FLUSH {\n            return Ok(Plan {\n"
plants = [
  ("C1 the old unchecked product", ctl, fix,
   "        let at = || -> io::Result<u64> { Ok(f.desc.start_sector * 512) };\n"),
  ("C2 saturating_mul in place of checked_mul", ctl, fix,
   "        let at = || -> io::Result<u64> { Ok(f.desc.start_sector.saturating_mul(512)) };\n"),
  ("C3 every request's place checked, a flush's too", ctl, fix,
   fix + "        if at().is_err() {\n            queue.commit(f.tag, -libc::EIO, None);\n            return;\n        }\n"),
  ("D1 a flush's range is checked as any request's", tr, flush_ret,
   "        if op == op::FLUSH {\n            self.range(desc, false)?;\n        }\n" + flush_ret),
  ("D2 a flush must start inside the device", tr, flush_ret,
   "        if op == op::FLUSH && desc.start_sector.checked_mul(SECTOR).is_none_or(|b| b >= self.size) {\n            return Err(Refusal::OutOfBounds { start: desc.start_sector, sectors: desc.nr_sectors });\n        }\n" + flush_ret),
  ("D3 a flush's byte is computed unchecked", tr, flush_ret,
   "        if op == op::FLUSH {\n            std::hint::black_box(desc.start_sector * SECTOR);\n        }\n" + flush_ret),
]
def test(pkg_args):
    p = subprocess.run(["cargo", "test", "--locked", "-q"] + pkg_args, cwd=root, capture_output=True, text=True,
                       env=dict(os.environ, CARGO_TARGET_DIR=target), timeout=1800)
    out = p.stdout + p.stderr
    failed = sorted(set(re.findall(r"^    (\S+)$", out, re.M)))
    err = "" if p.returncode == 0 or failed else " BUILD/OTHER: " + " | ".join(l for l in out.splitlines() if l.startswith("error"))[:300]
    return p.returncode, failed, err
for name, path, a, b in plants:
    f = root / path; orig = f.read_text()
    assert orig.count(a) == 1, name
    try:
        f.write_text(orig.replace(a, b, 1))
        args = ["-p", "skyzfs-ublk-sys", "--features", "control-test-program", "--bin", "skyzfs-ublk-control"] if path == ctl else ["-p", "skyzfs-ublk", "--lib"]
        rc, failed, err = test(args)
        print(f"{name}: rc {rc}; failed: {', '.join(failed) or 'none'}{err}", flush=True)
    finally:
        f.write_text(orig)
