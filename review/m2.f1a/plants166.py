"""Plants of the review of #166 (F1(a)), head 7d98b26. Each is applied
alone, as text, to a worktree of the head that also holds the reviewer's
scratch tests (`mod review166` inside `control/tests.rs`'s `mod socket`);
the suite runs with a 600 s limit; then the files are restored
(try/finally).

  KILLED       a test failed (the failing tests are listed, the PR's
               and the reviewer's apart)
  SURVIVED     every test passed
  NOT APPLIED  the text to replace was not found exactly once
  TIMEOUT      the suite ran past 600 s (counted as caught, and said)
  NO BUILD     the plant does not compile

Suite: `cargo test -p skyzfsd --lib control::` (the PR's control tests
and the reviewer's). With --repeat N TEST, the one test is run N times
on the plant (to measure a racy kill).

WORKTREE=<path> CARGO_TARGET_DIR=<path> python3 -I plants166.py [ids...]
"""
import os, re, subprocess, sys, time

W = os.environ["WORKTREE"]
CTL = "bins/skyzfsd/src/control.rs"
SRV = "bins/skyzfsd/src/control/server.rs"

SUITE = ["cargo", "test", "-p", "skyzfsd", "--lib", "control::", "--", "--test-threads=4"]

GUARD = """        Ok(_) if hung_up() => {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the client hung up before its request was handled",
            ))
        }
        Ok((request, manifest)) => handler(&request, manifest),
    };
"""
POLLFD = "let mut fds = [PollFd::new(stream, PollFlags::empty())];"
TAKEN = "Ok(_) => fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR),"
WIRED = "if let Ok(reply) = answer(&mut timed, handler, || hung_up(&stream)) {"
COUNTED = """        if let Ok(reply) = answer(&mut timed, handler, || hung_up(&stream)) {
            counts.answered.fetch_add(1, Ordering::Relaxed);
"""
READ = "    let reply = match read_request(&mut reader)? {\n        Err(reply) => reply,\n"

P = [
    # The author's plants, as the description names them.
    ("P1", "the client not looked at (the guard never true)",
     [(CTL, "Ok(_) if hung_up() => {", "Ok(_) if false => {")]),
    ("P2", "POLLRDHUP asked for and taken as hung up",
     [(SRV, POLLFD, "let mut fds = [PollFd::new(stream, PollFlags::RDHUP)];"),
      (SRV, TAKEN, "Ok(_) => fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::RDHUP),")]),
    ("P3", "only an error taken as hung up",
     [(SRV, TAKEN, "Ok(_) => fds[0].revents().intersects(PollFlags::ERR),")]),
    ("P4", "looked at after the handler",
     [(CTL, GUARD, """        Ok((request, manifest)) => {
            let reply = handler(&request, manifest);
            if hung_up() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "the client hung up before its request was handled",
                ));
            }
            reply
        }
    };
""")]),
    ("P5", "refusals look at the client too",
     [(CTL, READ, """    let read = read_request(&mut reader)?;
    if hung_up() {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the client hung up before its request was handled",
        ));
    }
    let reply = match read {
        Err(reply) => reply,
"""),
      (CTL, GUARD, """        Ok((request, manifest)) => handler(&request, manifest),
    };
""")]),
    # The reviewer's.
    ("Q1", "the server passes a check that always says present",
     [(SRV, WIRED, "if let Ok(reply) = answer(&mut timed, handler, || false) {")]),
    ("Q2", "POLLRDHUP taken without being asked for (Linux does not report it)",
     [(SRV, TAKEN, "Ok(_) => fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::RDHUP),")]),
    ("Q3", "only POLLHUP taken (POLLERR dropped)",
     [(SRV, TAKEN, "Ok(_) => fds[0].revents().intersects(PollFlags::HUP),")]),
    ("Q4", "POLLIN asked for and taken as hung up (readable = gone)",
     [(SRV, POLLFD, "let mut fds = [PollFd::new(stream, PollFlags::IN)];"),
      (SRV, TAKEN, "Ok(_) => fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::IN),")]),
    ("Q5", "the client looked at before the request is read",
     [(CTL, READ, """    let gone = hung_up();
    let reply = match read_request(&mut reader)? {
        Err(reply) => reply,
"""),
      (CTL, "Ok(_) if hung_up() => {", "Ok(_) if gone => {")]),
    ("Q6", "a poll that fails taken as hung up",
     [(SRV, "        Err(_) => false,\n    }\n}", "        Err(_) => true,\n    }\n}")]),
    ("Q7", "a dropped request counted as answered",
     [(SRV, COUNTED, """        let answered = answer(&mut timed, handler, || hung_up(&stream));
        if matches!(&answered, Err(e) if e.kind() == io::ErrorKind::ConnectionAborted) {
            counts.answered.fetch_add(1, Ordering::Relaxed);
            post(counts.figures());
        }
        if let Ok(reply) = answered {
            counts.answered.fetch_add(1, Ordering::Relaxed);
""")]),
    ("Q8", "a dropped request answered (refused `ended`) instead of left unanswered",
     [(CTL, """        Ok(_) if hung_up() => {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the client hung up before its request was handled",
            ))
        }
""", """        Ok(_) if hung_up() => Reply::Refused(Code::Ended, "the client hung up".into()),
""")]),
    ("Q9", "the poll's timeout made 50 ms (a wait, not a look)",
     [(SRV, "        tv_nsec: 0,\n    };\n    match poll", "        tv_nsec: 50_000_000,\n    };\n    match poll")]),
    # A timing probe, not a change of behaviour: the count is made 300 ms
    # after the answer is written. A test that reads the figures right
    # after its client's answer, with no post to wait on, then fails.
    ("T1", "timing probe: the answer counted 300 ms after it is written",
     [(SRV, COUNTED, """        if let Ok(reply) = answer(&mut timed, handler, || hung_up(&stream)) {
            std::thread::sleep(Duration::from_millis(300));
            counts.answered.fetch_add(1, Ordering::Relaxed);
""")]),
]


def run(cmd, extra=()):
    t = time.time()
    try:
        r = subprocess.run(list(cmd) + list(extra), cwd=W, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return "TIMEOUT", time.time() - t, [], ""
    out = r.stdout + r.stderr
    failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")]
    passed = len([l for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith(" ok")])
    if r.returncode == 0:
        return "pass", time.time() - t, [], f"{passed} passed"
    if "error[E" in out or "error: could not compile" in out:
        errs = [l for l in out.splitlines() if l.startswith("error")][:3]
        return "NO BUILD", time.time() - t, [], " | ".join(errs)
    return "fail", time.time() - t, failed, f"{passed} passed"


def apply(edits):
    files = sorted({f for f, _, _ in edits})
    saved = {f: open(os.path.join(W, f)).read() for f in files}
    texts = dict(saved)
    for f, old, new in edits:
        n = texts[f].count(old)
        if n != 1:
            return None, saved, f"NOT APPLIED ({f}: found {n})"
        texts[f] = texts[f].replace(old, new)
    return texts, saved, None


args = sys.argv[1:]
repeat = None
if args and args[0] == "--repeat":
    repeat = (int(args[1]), args[2])
    args = args[3:]
want = args
for pid, what, edits in P:
    if want and pid not in want:
        continue
    texts, saved, bad = apply(edits)
    if bad:
        print(f"{pid} {what}: {bad}", flush=True)
        continue
    try:
        for f in texts:
            open(os.path.join(W, f), "w").write(texts[f])
        if repeat:
            n, test = repeat
            # Build once, then run the one test N times.
            v, s, failed, note = run(["cargo", "test", "-p", "skyzfsd", "--lib", "--no-run"])
            kills = 0
            for i in range(n):
                v, s, failed, note = run(["cargo", "test", "-p", "skyzfsd", "--lib", test, "--", "--exact"])
                if v != "pass":
                    kills += 1
            print(f"{pid} {what}: {test} failed {kills} of {n} runs", flush=True)
            continue
        verdict, secs, failed, note = run(SUITE)
    finally:
        for f in saved:
            open(os.path.join(W, f), "w").write(saved[f])
    mine = [t for t in failed if "review166" in t]
    theirs = [t for t in failed if "review166" not in t]
    if verdict == "NO BUILD":
        v = "NO BUILD"
    elif verdict == "TIMEOUT":
        v = "TIMEOUT (caught)"
    elif verdict == "pass":
        v = "SURVIVED"
    else:
        v = "KILLED" if theirs else "SURVIVED the PR's tests; KILLED by the reviewer's"
    short = lambda ts: ", ".join(t.split("::")[-1] for t in ts) or "-"
    print(f"{pid} {what}: {v} [{secs:.0f}s; {note}; PR's failing: {short(theirs)}; reviewer's failing: {short(mine)}]", flush=True)
