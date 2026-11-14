import subprocess, sys, pathlib
root = pathlib.Path(sys.argv[1])
rig = root / "tests/kernel/control-rig.sh"
orig = rig.read_text()
plants = {
  "T1 the take-over's own end is not looked at": ('        if ! kill -0 "$SRV_PID" 2>/dev/null; then\n', '        if false; then\n'),
  "T2 a take-over that ended by itself is named but not failed": ('            wait "$SRV_PID" 2>/dev/null; v=$?\n            r=FAIL\n', '            wait "$SRV_PID" 2>/dev/null; v=$?\n'),
}
def run():
    p = subprocess.run(["bash", "tests/kernel/test_control_refused.sh"], cwd=root, capture_output=True, text=True, timeout=600)
    fails = [l for l in p.stdout.splitlines() + p.stderr.splitlines() if l.startswith("FAIL")]
    return p.returncode, fails
try:
    for name, (a, b) in plants.items():
        assert orig.count(a) == 1, name
        rig.write_text(orig.replace(a, b))
        rc, fails = run()
        print(name, "rc", rc, [f[:110] for f in fails])
finally:
    rig.write_text(orig)
rc, fails = run()
print("clean rc", rc, fails)
