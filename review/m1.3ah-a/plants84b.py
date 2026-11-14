"""Plant one change at a time in the #84 worktree, run the tests, restore."""
import os, re, subprocess, sys
W = "/home/user/skyzfs-pr84"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
Q = "crates/skyzfs-ublk-sys/src/queue.rs"
P = "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
PLANTS = [
    ("C1 own_control_ring makes nothing", Q,
     [("        if ring.is_none() {\n            let made", "        if false && ring.is_none() {\n            let made")]),
    ("C2 own_control_ring always makes a new ring", Q,
     [("        if ring.is_none() {\n            let made", "        if true || ring.is_none() {\n            let made")]),
    ("C3 the program's start thread calls start(), not the entry point", P,
     [("let started = device.start_from_another_thread_for_tests();", "let started = device.start();")]),
    ("C4 futex and io_uring_enter swapped", P,
     [("Ok(libc::SYS_io_uring_enter) => \"io_uring_enter\".into(),\n                Ok(libc::SYS_futex) => \"futex\".into(),",
       "Ok(libc::SYS_io_uring_enter) => \"futex\".into(),\n                Ok(libc::SYS_futex) => \"io_uring_enter\".into(),")]),
    ("C5 taken answered as any other (the old code)", P,
     [("                    counts.failed.fetch_add(1, Ordering::Relaxed);\n                    queue.commit(f.tag, -libc::EIO, None);\n                    continue;\n", "")]),
    ("C6 taken not counted as failed", P,
     [("                    counts.failed.fetch_add(1, Ordering::Relaxed);\n                    queue.commit(f.tag, -libc::EIO, None);", "                    queue.commit(f.tag, -libc::EIO, None);")]),
    ("C7 the start thread's id never sent", P,
     [("            let _ = named.send(own_tid());\n", "            let _ = named.send(None);\n")]),
    ("C8 waits_in reads stat, not syscall", P,
     [("syscall_name(&read(\"syscall\"))", "syscall_name(&read(\"stat\"))")]),
    ("C9 taken answered with EIO but the tag kept as refused (said again)", P,
     [("                    refusing_fetch = None;\n                    say(\"refuse-fetch\", \"taken\");", "                    say(\"refuse-fetch\", \"taken\");")]),
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
