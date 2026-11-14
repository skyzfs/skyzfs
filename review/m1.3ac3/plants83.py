"""Plant one change at a time in the #83 worktree, run the tests, restore."""
import subprocess, sys, os
W = "/home/user/skyzfs-pr83"
ENV = dict(os.environ, CARGO_TARGET_DIR="/home/user/target-pr74")
PUMP = "crates/skyzfs-ublk/src/pump.rs"
SERVE = "bins/skyzfsd/src/serve.rs"
REF = "bins/skyzfsd/src/refuse.rs"
CHECK = """        if waited == Err(QueueDown) {
            return Some(Stopped::QueueDown);
        }
        // Before anything of this wait is taken: what it handed over is
        // the next server's.
        if let Some(refused) = self.refused_commit(&unfetched) {
            return Some(refused);
        }
"""
REFUSAL = """        if let Some(refused) = self.refused_commit(&unfetched) {
            return Some(refused);
        }
"""
PLANTS = [
    ("P1 the last refused commit of a wait is named", PUMP,
     [(".iter()\n            .find(|command| command.command == Command::Commit",
       ".iter()\n            .rev()\n            .find(|command| command.command == Command::Commit")]),
    ("P2 the refusal is checked before QueueDown", PUMP,
     [(CHECK, REFUSAL + "        if waited == Err(QueueDown) {\n            return Some(Stopped::QueueDown);\n        }\n")]),
    ("P3 a failed volume's turn ends on a refused commit", PUMP,
     [("        self.count_unfetched(&unfetched);\n        let woken = match waited {",
       "        self.count_unfetched(&unfetched);\n        if let Some(r) = self.refused_commit(&unfetched) {\n            return Some(r);\n        }\n        let woken = match waited {")]),
    ("P4 the refusal checked after the wait's requests are taken, before submit", PUMP,
     [(CHECK, "        if waited == Err(QueueDown) {\n            return Some(Stopped::QueueDown);\n        }\n"),
      ("        // Completions make room, and room lets more be submitted.\n",
       REFUSAL + "        // Completions make room, and room lets more be submitted.\n")]),
    ("P5 an abort is taken for a refusal", PUMP,
     [("command.command == Command::Commit && command.result != ABORTED",
       "command.command == Command::Commit")]),
    ("P6 a refused fetch ends the pump too", PUMP,
     [("command.command == Command::Commit && command.result != ABORTED",
       "command.result != ABORTED")]),
    ("P7 a refusal is ignored while the pump drains for a stop", PUMP,
     [(REFUSAL + "        if waited == Ok(Wake::Stop) {",
       "        if !self.stopping {\n" + REFUSAL + "        }\n        if waited == Ok(Wake::Stop) {")]),
    ("P8 errno not negated", PUMP,
     [("errno: command.result.saturating_neg(),", "errno: command.result,")]),
    ("P9 a refusal during an asked stop exits 0", SERVE,
     [("        Stopped::Orderly if asked_to_stop => Ok(Exit::Done),",
       "        Stopped::Orderly if asked_to_stop => Ok(Exit::Done),\n        Stopped::CommitRefused { .. } if asked_to_stop => Ok(Exit::Done),")]),
    ("P10 the hook refuses every request of the kind", REF,
     [("                self.tag = Some(first.tag);\n                self.what = None;",
       "                self.tag = Some(first.tag);")]),
    ("P11 the hook also sends the commit with its buffer", REF,
     [("                self.kept.push(buf);\n                return;", "                self.kept.push(None);")]),
    ("N1 at a stop's end the refusal comes before QueueDown", PUMP,
     [("""        self.count_unfetched(&unfetched);
        if flushed.is_err() {
            return Some(Stopped::QueueDown);
        }
        // A commit the driver refused when the flush sent it left its
        // request unanswered: the stop is not orderly, as for a refusal
        // that a wait brings.
        if let Some(refused) = self.refused_commit(&unfetched) {
            return Some(refused);
        }
""", """        self.count_unfetched(&unfetched);
        if let Some(refused) = self.refused_commit(&unfetched) {
            return Some(refused);
        }
        if flushed.is_err() {
            return Some(Stopped::QueueDown);
        }
""")]),
    ("N2 what a failed flush reaps is not counted", PUMP,
     [("""        self.count_unfetched(&unfetched);
        if flushed.is_err() {
            return Some(Stopped::QueueDown);
        }
""", """        if flushed.is_err() {
            return Some(Stopped::QueueDown);
        }
        self.count_unfetched(&unfetched);
""")]),
    ("N3 a failed volume's last flush is not counted", PUMP,
     [("""            let _ = self.queue.flush(&mut unfetched);
            self.count_unfetched(&unfetched);
""", """            let _ = self.queue.flush(&mut unfetched);
""")]),
    ("N4 the kernel queue's flush hands nothing on", "crates/skyzfs-ublk-sys/src/queue.rs",
     [("""                    if let Some(Err(command)) = ended(user_data, done.result()) {
                        unfetched.push(command);
                    }""", """                    let _ = (user_data, done, &unfetched);""")]),
    ("N5 the record writes no ended line at a flush", "bins/skyzfsd/src/record.rs",
     [("""        self.sent(flushed.is_ok());
        self.ended(unfetched.get(ended_before..).unwrap_or_default());
        flushed""", """        self.sent(flushed.is_ok());
        let _ = ended_before;
        flushed""")]),
    ("N6 the control program does not read its last flush", "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs",
     [("""                let _ = queue.flush(&mut unfetched);
                if note_ended(&unfetched, &mut refusing, &counts, &say) {
                    counts.refused.store(true, Ordering::SeqCst);
                }""", """                let _ = queue.flush(&mut unfetched);""")]),
]
TESTS = {PUMP: ["cargo", "test", "-q", "-p", "skyzfs-ublk", "--lib", "pump::"],
         SERVE: ["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib", "serve::"],
         REF: ["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib"],
         "crates/skyzfs-ublk-sys/src/queue.rs": ["cargo", "test", "-q", "-p", "skyzfs-ublk-sys", "--features", "control-test-program"],
         "bins/skyzfsd/src/record.rs": ["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib"],
         "crates/skyzfs-ublk-sys/src/bin/skyzfs-ublk-control.rs": ["cargo", "test", "-q", "-p", "skyzfs-ublk-sys", "--features", "control-test-program"]}
only = sys.argv[1:]
for name, path, edits in PLANTS:
    if only and name.split()[0] not in only:
        continue
    p = os.path.join(W, path)
    text = open(p).read()
    new = text
    ok = True
    for old, rep in edits:
        if new.count(old) != 1:
            print(f"{name}: anchor found {new.count(old)} times"); ok = False; break
        new = new.replace(old, rep)
    if not ok:
        continue
    open(p, "w").write(new)
    try:
        cmds = [TESTS[path]]
        if path == PUMP:
            cmds.append(["cargo", "test", "-q", "-p", "skyzfsd", "--features", "test-hooks", "--lib", "serve::"])
        lines = []
        for cmd in cmds:
            r = subprocess.run(cmd, cwd=W, env=ENV, capture_output=True, text=True)
            out = r.stdout + r.stderr
            import re
            failed = sorted(set(re.findall(r"^---- (\S+) stdout ----", out, re.M)))
            if r.returncode != 0 and not failed:
                failed = ["(build or run error) " + out.strip().splitlines()[-1][:200] if out.strip() else "(error)"]
            lines.append(f"{' '.join(cmd[3:])}: rc {r.returncode}; failed: {', '.join(failed) or 'none'}")
        print(name); [print("   ", l) for l in lines]
    finally:
        subprocess.run(["git", "checkout", "--", path], cwd=W)
print("worktree clean:", subprocess.run(["git", "status", "--short"], cwd=W, capture_output=True, text=True).stdout.strip() == "")
