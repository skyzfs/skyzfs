import subprocess, sys, os, json, time
W = os.environ.get("WORKTREE", ".")  # a worktree of the head, restored after each plant
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
def skyzfsd(filt):
    return ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", filt, "--", "--skip", "review"]
def upload_t(filt):
    return ["cargo", "test", "-p", "skyzfs-upload", "--test", "uploader", filt, "--", "--skip", "review"]
def ublk(filt):
    return ["cargo", "test", "-p", "skyzfs-ublk", "--lib", filt]
P = [
 ("M1 peer check lets any user in", "bins/skyzfsd/src/control/server.rs",
  "uid == 0 || uid == rustix::process::geteuid().as_raw()",
  "uid == 0 || uid == rustix::process::geteuid().as_raw() || uid != 0", skyzfsd("control")),
 ("M2 socket left with the umask's mode", "bins/skyzfsd/src/control/server.rs",
  "fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;",
  "let _ = fs::Permissions::from_mode(0o600);", skyzfsd("control")),
 ("M3 a line of exactly LINE_MAX refused", "bins/skyzfsd/src/control.rs",
  ".take(LINE_MAX as u64)", ".take(LINE_MAX as u64 - 1)", skyzfsd("control")),
 ("M4 a full inbox answered ended", "bins/skyzfsd/src/control/handler.rs",
  "mpsc::error::TrySendError::Full(_) => Reply::Refused(", "mpsc::error::TrySendError::Full(_) => ended(), mpsc::error::TrySendError::Full(_) if false => Reply::Refused(", skyzfsd("control")),
 ("M5 the reason for another holder dropped", "bins/skyzfsd/src/serve.rs",
  "Some((AtStart::Another { holder, epoch }, _)) => format!(", "Some((AtStart::Another { holder, epoch }, _)) if false => format!(", skyzfsd("serve")),
 ("M6 version refusals not counted malformed", "bins/skyzfsd/src/control/server.rs",
  "Reply::Refused(Code::Malformed | Code::Version, _)", "Reply::Refused(Code::Malformed, _)", skyzfsd("control")),
 ("M7 a refused publish answers exit 0", "bins/skyzfsd/src/commands.rs",
  "exit: if reply.refused() {", "exit: if false && reply.refused() {", skyzfsd("commands")),
 ("M8 newest always opens as itself", "bins/skyzfsd/src/control/handler.rs",
  "newest: now.newest.map(|cut| (cut.group, cut.itself)),", "newest: now.newest.map(|cut| (cut.group, true)),", skyzfsd("control")),
 ("M9 take_ended does not drain", "crates/skyzfs-upload/src/publish.rs",
  "self.ended.drain(..).collect()", "self.ended.iter().cloned().collect()", upload_t("")),
 ("M10 the attachment not told to the uploader", "bins/skyzfsd/src/upload.rs",
  "uploader.publishing().set_attachment(attachment);", "let _ = attachment;", skyzfsd("upload")),
 ("M11 a peer refusal not written", "bins/skyzfsd/src/control/server.rs",
  "let _ = stream.write_all(format!(\"{HELLO}\\n{}\\n\", refusal.line()).as_bytes());", "let _ = &refusal;", skyzfsd("control")),
 ("M12 an asked line with G and C swapped", "bins/skyzfsd/src/record.rs",
  "(\"asked\", group.to_string(), after.to_string())", "(\"asked\", after.to_string(), group.to_string())", skyzfsd("record")),
 ("M13 the store's state not read at an ask (460e67c undone)", "crates/skyzfs-upload/src/uploader.rs",
  "                if let Some(state) = state {\n                    self.publishing().set_state(state);\n                }\n", "                let _ = state;\n", upload_t("")),
 ("M14 the early expiry refusal dropped", "bins/skyzfsd/src/control.rs",
  "if p.expiry.is_some_and(|e| e.is_zero() || e > EXPIRY_MAX) {", "if false && p.expiry.is_some_and(|e| e.is_zero() || e > EXPIRY_MAX) {", skyzfsd("control")),
 ("A-C3 a failed channel not posted failed", "bins/skyzfsd/src/serve.rs",
  "            post(ChannelShown {\n                state: ChannelStateShown::Failed,\n                ..ChannelShown::default()\n            });\n", "", skyzfsd("serve")),
 ("A-K2 publish:N counts a refused publish", "bins/skyzfsd/src/kill.rs",
  "        self.inner.publish(commit, snapshot).await?;\n        self.publishes += 1;\n", "        self.publishes += 1;\n        self.inner.publish(commit, snapshot).await?;\n", skyzfsd("kill")),
 ("A-U2 the status leaves the publishes out", "bins/skyzfsd/src/upload.rs",
  "out.publish = Some(publish_shown(&shown.publishes));", "out.publish = None; let _ = publish_shown;", skyzfsd("upload")),
 ("A-S2 a v17 file read with a channel", "crates/skyzfs-ublk/src/status.rs",
  "let channel = (version >= 18).then(", "let channel = (version >= 17).then(", ublk("status")),
 ("A-R2 a withdrawal has no line", "bins/skyzfsd/src/record.rs",
  "Request::Withdraw(name) => lines.publish(PublishLine::Withdraw, name, &reply),", "Request::Withdraw(_) => {}", skyzfsd("record")),
]
only = sys.argv[1:]
res = []
for name, f, old, new, cmd in P:
    if only and not any(name.startswith(o) for o in only):
        continue
    path = os.path.join(W, f)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})")); print(name, "NOT APPLIED", n, flush=True); continue
    try:
        open(path, "w").write(text.replace(old, new))
        t = time.time()
        try:
            r = subprocess.run(cmd, cwd=W, env=ENV, capture_output=True, text=True, timeout=1500)
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
            elif r.returncode == 0:
                verdict = "SURVIVED"
            else:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = "caught: " + ", ".join(failed[:4])
        except subprocess.TimeoutExpired:
            verdict = "caught (timeout)"
        res.append((name, verdict))
        print(f"{name}: {verdict} ({time.time()-t:.0f}s)", flush=True)
    finally:
        open(path, "w").write(text)
json.dump(res, open("plants-%d.json" % os.getpid(), "w"), indent=1)
