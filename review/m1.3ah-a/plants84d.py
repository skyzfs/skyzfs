"""Plant one change at a time in the #84 worktree, run the tests, restore."""
import os, re, subprocess, sys
W = "/home/user/skyzfs-pr84"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
Q = "crates/skyzfs-ublk-sys/src/queue.rs"
P = "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
PLANTS = [
    ("D2 workers_in reads status, not comm", P,
     [("(read(tid, \"comm\"), read(tid, \"wchan\"))", "(read(tid, \"status\"), read(tid, \"wchan\"))")]),
    ("D3 workers_in always says none", P,
     [("        workers_named(&found)\n", "        let _ = workers_named(&found);\n        \"none\".into()\n")]),
    ("E1 the ids sorted as text", P,
     [("        let mut tids: Vec<u32> = tasks", "        let mut tids: Vec<String> = tasks"),
      ("            .filter_map(|task| task.file_name().to_str()?.parse().ok())\n            .collect();\n        tids.sort_unstable();",
       "            .filter_map(|task| task.file_name().to_str().filter(|n| n.parse::<u32>().is_ok()).map(String::from))\n            .collect();\n        tids.sort_unstable();"),
      ("        let read = |tid: u32, what: &str| {\n            std::fs::read_to_string(dir.join(tid.to_string()).join(what))",
       "        let read = |tid: String, what: &str| {\n            std::fs::read_to_string(dir.join(&tid).join(what))"),
      ("            .map(|tid| (read(tid, \"comm\"), read(tid, \"wchan\")))",
       "            .map(|tid| (read(tid.clone(), \"comm\"), read(tid, \"wchan\")))")]),
    ("E2 the start thread's id not said", P,
     [("Some(tid) => format!(\"its thread {tid} waits in {}\", waits_in(tid)),",
       "Some(tid) => format!(\"its thread waits in {}\", waits_in(tid)),")]),
    ("E3 the workers in the order /proc gives", P,
     [("        tids.sort_unstable();\n", "")]),
    ("E4 workers_wait reads another directory", P,
     [("workers_in(std::path::Path::new(\"/proc/self/task\"))", "workers_in(std::path::Path::new(\"/proc/self\"))")]),
]
only = sys.argv[1:]
cmd = ["cargo", "test", "-q", "-p", "skyzfs-ublk-sys", "--features", "control-test-program"]
for name, path, edits in PLANTS:
    if only and name.split()[0] not in only:
        continue
    p = os.path.join(W, path)
    text = open(p).read()
    new = text
    bad = False
    for old, rep in edits:
        if new.count(old) != 1:
            print(f"{name}: anchor found {new.count(old)} times"); bad = True; break
        new = new.replace(old, rep)
    if bad:
        continue
    open(p, "w").write(new)
    try:
        r = subprocess.run(cmd, cwd=W, env=ENV, capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted(set(re.findall(r"^---- (\S+) stdout ----", out, re.M)))
        if r.returncode != 0 and not failed:
            errs = [l for l in out.splitlines() if l.startswith("error")]
            failed = ["(build error) " + (errs[0][:160] if errs else out.strip().splitlines()[-1][:160])]
        print(f"{name}: rc {r.returncode}; failed: {', '.join(failed) or 'none'}")
    finally:
        subprocess.run(["git", "checkout", "--", path], cwd=W)
print("worktree clean:", subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout.strip() == "")
