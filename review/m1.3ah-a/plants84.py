"""Plant one change at a time in the #84 worktree, run the tests, restore."""
import os, re, subprocess, sys
W = "/home/user/skyzfs-pr84"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr73")
Q = "crates/skyzfs-ublk-sys/src/queue.rs"
P = "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs"
PLANTS = [
    ("A1 the fetch's code never ioctl-encoded", Q,
     [("    let code = if encoded {\n        sys::UBLK_U_IO_FETCH_REQ\n    } else {",
       "    let code = if false && encoded {\n        sys::UBLK_U_IO_FETCH_REQ\n    } else {")]),
    ("A2 the payload's result 0, not -1", Q,
     [("bytes[4..8].copy_from_slice(&(-1i32).to_ne_bytes());", "bytes[4..8].copy_from_slice(&0i32.to_ne_bytes());")]),
    ("A3 the payload names queue 1", Q,
     [("    let mut bytes = [0; 16];\n", "    let mut bytes = [0; 16];\n    bytes[0] = 1;\n")]),
    ("A4 the user data names a commit", Q,
     [(".user_data(OWN_FETCH | UblkIOCtx::build_user_data(tag, sys::UBLK_U_IO_FETCH_REQ, 0, false))",
       ".user_data(OWN_FETCH | UblkIOCtx::build_user_data(tag, sys::UBLK_U_IO_COMMIT_AND_FETCH_REQ, 0, false))")]),
    ("A5 the buffers are not registered first", Q,
     [("    let queue = queue.regiser_io_bufs(Some(first));\n", "    let _ = first;\n")]),
    ("A6 the refused tag names its buffer", Q,
     [("    if tag == refused {\n        return Ok(0);\n    }", "    let _ = refused;")]),
    ("A7 OWN_FETCH has bit 62 only", Q,
     [("const OWN_FETCH: u64 = (1 << 63) | (1 << 62);", "const OWN_FETCH: u64 = 1 << 62;")]),
    ("A8 the fetches in reverse tag order", Q,
     [("    for tag in queue.tags() {\n        let addr = first_address", "    for tag in queue.tags().rev() {\n        let addr = first_address")]),
    ("B1 the sent line is not said", P,
     [("        if let Some(tag) = refusing_fetch {\n            say(\"refuse-fetch\", &format!(\"sent on tag {tag}\"));\n        }\n", "")]),
    ("B2 a refused fetch ends the server", P,
     [("                        say(\"refuse-fetch\", &format!(\"refused: {why}\"));\n",
       "                        say(\"refuse-fetch\", &format!(\"refused: {why}\"));\n                        refused = true;\n")]),
    ("B3 any tag's refused fetch is said", P,
     [("if refusing_fetch == Some(command.tag) && command.command == Command::Fetch {",
       "if refusing_fetch.is_some() && command.command == Command::Fetch {")]),
    ("B4 the fetch option goes with --delete-after-ms", P,
     [("if refused.is_some() || delete_after.is_some() {", "if refused.is_some() {")]),
    ("B5 the value is not checked against the step", P,
     [("if (then == Then::Recovery) != take_over {", "if false && (then == Then::Recovery) != take_over {")]),
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
