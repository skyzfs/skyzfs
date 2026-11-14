"""Plant one change at a time in the #84 worktree, run the tests, restore."""
import os, re, subprocess, sys
W = "/home/user/skyzfs-pr84"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
Q = "crates/skyzfs-ublk-sys/src/queue.rs"
P = "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
FLUSH = "                let _ = queue.flush(&mut unfetched);\n                if note_ended(\n                    &unfetched,\n                    &mut refusing,\n                    &mut refusing_fetch,\n"
WAIT = "            if note_ended(\n                &unfetched,\n                &mut refusing,\n                &mut refusing_fetch,\n"
PLANTS = [
    ("C7 the start thread's id never sent", P,
     [("            let _ = named.send(own_tid());\n", "            let _ = named.send(None);\n")]),
    ("C8 waits_in reads stat, not syscall", P,
     [("syscall_name(&task_file(tid, \"syscall\"))", "syscall_name(&task_file(tid, \"stat\"))")]),
    ("C9 after taken the tag is still watched", P,
     [("                    refusing_fetch = None;\n                    say(\"refuse-fetch\", \"taken\");", "                    say(\"refuse-fetch\", \"taken\");")]),
    ("D1 every iou- thread taken for a worker", P,
     [("comm.trim().starts_with(\"iou-wrk-\")", "comm.trim().starts_with(\"iou-\")")]),
    ("D2 workers_wait reads status, not comm", P,
     [("(task_file(tid, \"comm\"), task_file(tid, \"wchan\"))", "(task_file(tid, \"status\"), task_file(tid, \"wchan\"))")]),
    ("D3 workers_wait always says none", P,
     [("        workers_named(&found)\n", "        let _ = workers_named(&found);\n        \"none\".into()\n")]),
    ("D4 the last flush does not read the refused fetch", P,
     [(FLUSH, FLUSH.replace("&mut refusing_fetch,", "&mut None,"))]),
    ("D5 a refused fetch ends the server", P,
     [("                    say(\"refuse-fetch\", &format!(\"refused: {why}\"));\n",
       "                    say(\"refuse-fetch\", &format!(\"refused: {why}\"));\n                    refused = true;\n")]),
    ("D6 the last flush does not read the refused commit", P,
     [(FLUSH, FLUSH.replace("&mut refusing,", "&mut None,"))]),
    ("D7 a wait does not read the refused commit", P,
     [(WAIT, WAIT.replace("&mut refusing,", "&mut None,"))]),
    ("D8 a wait does not read the refused fetch", P,
     [(WAIT, WAIT.replace("&mut refusing_fetch,", "&mut None,"))]),
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
