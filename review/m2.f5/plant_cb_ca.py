import os, subprocess, sys
W = sys.argv[1]
T = "bins/skyzfsd/src/control/tests.rs"
C = "bins/skyzfsd/src/control.rs"
plants = {
 "CB": ("                std::thread::sleep(pause.min(left));\n", "                let _ = pause.min(left);\n"),
 "CA": ("            Err(Errno::AGAIN | Errno::INTR) => {\n", "            Err(Errno::AGAIN | Errno::INTR | Errno::CONNREFUSED | Errno::NOENT) => {\n"),
}
scen = open("scenarios_connect.rs").read()
t0 = open(os.path.join(W, T)).read(); c0 = open(os.path.join(W, C)).read()
try:
    open(os.path.join(W, T), "w").write(t0 + scen)
    for name, (old, new) in plants.items():
        assert c0.count(old) == 1, name
        try:
            open(os.path.join(W, C), "w").write(c0.replace(old, new))
            p = subprocess.run(["timeout", "600", "cargo", "test", "-p", "skyzfsd", "--features",
                "skyzfsd/test-hooks,skyzfs-local/test-hooks", "--lib", "review_connect", "--",
                "--nocapture", "--test-threads=1"], cwd=W, capture_output=True, text=True,
                env=dict(os.environ, CARGO_INCREMENTAL="0"))
            out = p.stdout + p.stderr
            failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
            lines = [l[l.index("## "):] for l in out.splitlines() if "## " in l]
            print(f"{name} with the scenarios: exit {p.returncode}; failed: {', '.join(failed) or 'none'}")
            for l in lines: print("   ", l)
        finally:
            open(os.path.join(W, C), "w").write(c0)
finally:
    open(os.path.join(W, T), "w").write(t0)
