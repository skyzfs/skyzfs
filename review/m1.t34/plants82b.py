"""Plants on e289b26's kill-loop fix; the tests run as nobody."""
import subprocess, sys, pathlib, re
root = pathlib.Path(sys.argv[1]); rig = root / "tests/kernel/daemon-rig.sh"; orig = rig.read_text()
wait_line = '            if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""; fi\n'
start = '        if ! kill -9 "$pid" 2>/dev/null; then\n'
end_ = '            break\n        fi\n        tk=$(now_ms)\n'
i, j = orig.index(start), orig.index(end_)
race = '''        if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ]; then
            KT_REFUSED=$(kt_refusal "$n" "$SERVE_RC" "$dir.serve.log" before)
            KT_ERR="before kill $n: the server had ended by itself, with exit $SERVE_RC"
            wait "$watcher"
            break
        fi
'''
plants = {
  "F1 the old line: a failed kill is only 'kill -9 failed'": orig[:i] + '        kill -9 "$pid" 2>/dev/null || { KT_ERR="kill $n: kill -9 $pid failed"; break; }\n        tk=$(now_ms)\n' + orig[j + len(end_):],
  "F2 the status is not read when the kill fails": orig.replace(wait_line, ""),
  "F3 the check after serve_wait (the race) removed": orig.replace(race, "") if orig.count(race) == 1 else None,
}
try:
    for name, text in plants.items():
        assert text and text != orig, name
        rig.write_text(text)
        p = subprocess.run(["runuser", "-u", "nobody", "--", "python3", "-I", "tests/kernel/test_daemon_check.py", "KillRefusal"],
                           cwd=root, capture_output=True, text=True, timeout=600)
        failed = sorted(set(re.findall(r"^(?:FAIL|ERROR): (\w+)", p.stderr, re.M)))
        print(f"{name}: rc {p.returncode}; failed: {', '.join(failed) or 'none'}", flush=True)
finally:
    rig.write_text(orig)
