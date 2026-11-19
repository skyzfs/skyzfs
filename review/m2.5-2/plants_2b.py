"""Second pass: A2d and A3d against the handler's tests alone, and the
plants that survived the pull request's tests against the review's
scenarios (appended to the test files for the run)."""
import os, subprocess, sys, time
sys.argv = [sys.argv[0], "__none__"]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "plants_2.py")).read().split("only = sys.argv[1:]")[0])
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
S2 = {
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", "--", "--skip", "review"],
    "ru": ["cargo", "test", "-p", "skyzfs-upload", "--test", "queued", "--test", "uploader", "review"],
    "rd": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", "review", "--", "--test-threads=1"],
}
RUNS = [("A2/A2d", "d"), ("A3/A3d", "d"), ("N3", "ru"), ("N4", "ru"), ("N6", "ru"), ("N5", "rd")]
for prefix, suite in RUNS:
    name, f, old, new, _ = next(p for p in P if p[0].startswith(prefix))
    path = os.path.join(W, f)
    text = open(path).read()
    assert text.count(old) == 1, name
    t = time.time()
    try:
        open(path, "w").write(text.replace(old, new))
        try:
            r = subprocess.run(S2[suite], cwd=W, env=ENV, capture_output=True, text=True, timeout=900)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = "SURVIVED"
            else:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = "caught: " + ", ".join(failed[:4])
        except subprocess.TimeoutExpired:
            verdict = "caught (timeout, a hang)"
    finally:
        open(path, "w").write(text)
    print(f"{name} [{suite}]: {verdict} ({time.time() - t:.0f}s)", flush=True)
print("done")
