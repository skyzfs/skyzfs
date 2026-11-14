"""Plants on a5796cc (#80), one at a time; the rig is restored after each."""
import subprocess, sys, pathlib
root = pathlib.Path(sys.argv[1]); rig = root / "tests/kernel/control-rig.sh"; orig = rig.read_text()
plants = {
 "A1 the take-over's count of the kind is not checked": ('            if [ "$n_kind" != 1 ]; then\n', '            if false; then\n'),
 "A2 the client is waited for even after the take-over ended": ('        if rc_client 60 "$SRV_PID"; then r=PASS; else r=FAIL; fi\n', '        if rc_client 60; then r=PASS; else r=FAIL; fi\n'),
 "A3 only the program's last line is quoted": ("    said=$(tail -n +\"$1\" \"$OUT/control.err\" 2>/dev/null | grep -A1 'panicked at' | tail -n 2 | tr '\\n' ' ')\n", "    said=\"\"\n"),
 "A4 the program's error is read from its first line": ('    errs=$(( $(wc -l < "$OUT/control.err" 2>/dev/null || echo 0) + 1 ))\n', '    errs=1\n'),
 "A5 a client that ended before the take-over is judged by it": ('    elif [ -e "$RC_RD.rc" ]; then\n', '    elif false; then\n'),
 "A6 a client's files are not cleared at the start": ('    rm -f "$RC_RD.rc" "$RC_RD.bin"      # a client\'s end is read from these\n', ''),
}
try:
    for name, (a, b) in plants.items():
        assert orig.count(a) == 1, name
        rig.write_text(orig.replace(a, b))
        p = subprocess.run(["bash", "tests/kernel/test_control_refused.sh"], cwd=root, capture_output=True, text=True, timeout=900)
        fails = [l[:140] for l in p.stdout.splitlines() + p.stderr.splitlines() if l.startswith("FAIL")]
        print(f"{name}: rc {p.returncode}; {len(fails)} failed", flush=True)
        for l in fails[:3]: print("   ", l)
finally:
    rig.write_text(orig)
