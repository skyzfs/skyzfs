"""Textual plants against the head, one at a time, each file restored
from its saved text after the run. Run from a worktree of the head:
WORKTREE=<path> python3 plants.py [names...]"""
import json, os, subprocess, sys, time

W = os.environ.get("WORKTREE", ".")
ENV = dict(os.environ, CARGO_INCREMENTAL="0")
FEAT = "skyzfsd/test-hooks,skyzfs-local/test-hooks"
SUITES = {
    "d": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", "--", "--skip", "review"],
    "u": ["cargo", "test", "-p", "skyzfs-upload", "--tests", "--lib", "--", "--skip", "review"],
    "b": ["cargo", "test", "-p", "skyzfs-ublk", "--lib"],
    "c": ["cargo", "test", "-p", "skyzfsd", "--features", FEAT, "--lib", "--", "--skip", "review"],
}
SV = "bins/skyzfsd/src/control/server.rs"
CT = "bins/skyzfsd/src/control.rs"
HD = "bins/skyzfsd/src/control/handler.rs"
SE = "bins/skyzfsd/src/serve.rs"
KI = "bins/skyzfsd/src/kill.rs"
RE = "bins/skyzfsd/src/record.rs"
UP = "bins/skyzfsd/src/upload.rs"
ST = "crates/skyzfs-ublk/src/status.rs"
QU = "crates/skyzfs-upload/src/queued.rs"
UL = "crates/skyzfs-upload/src/uploader.rs"
PB = "crates/skyzfs-upload/src/publish.rs"
P = [
 # The author's first round (C1 to S3), as described.
 ("C1 the channel posts nothing after an answer", SV,
  "                counts.malformed.fetch_add(1, Ordering::Relaxed);\n            }\n            post(counts.figures());\n",
  "                counts.malformed.fetch_add(1, Ordering::Relaxed);\n            }\n", "d"),
 ("C2 the channel posts nothing after a peer refused", SV,
  "                counts.peer.fetch_add(1, Ordering::Relaxed);\n                post(counts.figures());\n",
  "                counts.peer.fetch_add(1, Ordering::Relaxed);\n", "d"),
 ("C3 a channel that cannot be made is not posted failed", SE,
  "            post(ChannelShown {\n                state: ChannelStateShown::Failed,\n                ..ChannelShown::default()\n            });\n            None\n",
  "            None\n", "d"),
 ("C4 a channel with no uploader is posted as uploads", SE,
  "Err(why) => (handler::no_upload(why), no_upload),",
  "Err(why) => (handler::no_upload(why), ChannelStateShown::Uploads),", "d"),
 ("C5 a channel with an uploader is posted as no_upload", SE,
  "Ok(handler) => (handler, ChannelStateShown::Uploads),",
  "Ok(handler) => (handler, ChannelStateShown::NoUpload),", "d"),
 ("K1 manifest:N counts the ZFS manifest's PUT", KI,
  "Self::Manifest => key.contains(\"/snap/\"),",
  "Self::Manifest => key.contains(\"skyzfs/manifest/\"),", "d"),
 ("K2 publish:N counts a refused publish", KI,
  "        self.inner.publish(commit, snapshot).await?;\n        self.publishes += 1;\n",
  "        self.publishes += 1;\n        self.inner.publish(commit, snapshot).await?;\n", "d"),
 ("K3 publish:N falls before the transaction", KI,
  "        self.inner.publish(commit, snapshot).await?;\n        self.publishes += 1;\n        if self.publish_at == Some(self.publishes) {\n",
  "        self.publishes += 1;\n        if self.publish_at == Some(self.publishes) {\n            (self.die)(\"before\");\n        }\n        self.inner.publish(commit, snapshot).await?;\n        if self.publish_at == Some(self.publishes) {\n", "d"),
 ("K4 manifest:N not taken from the command line", "bins/skyzfsd/src/cli.rs",
  "\"manifest\" => Ok(KillAt::Manifest(n)),", "\"manifest-x\" => Ok(KillAt::Manifest(n)),", "d"),
 ("R1 a refusal's line carries its text", RE,
  "Reply::Refused(code, _) => format!(\"refused {}\", code.word()),",
  "Reply::Refused(..) => reply.line(),", "d"),
 ("R2 a withdrawal has no line", RE,
  "Request::Withdraw(name) => lines.publish(PublishLine::Withdraw, name, &reply),",
  "Request::Withdraw(_) => {}", "d"),
 ("U1 an ended publish is not told", UP,
  "                                told(name, progress);\n",
  "                                let _ = (name, progress, &mut told);\n", "d"),
 ("U2 the status leaves the publishes out", UP,
  "out.publish = Some(publish_shown(&shown.publishes));",
  "out.publish = None; let _ = publish_shown;", "d"),
 ("S1 the channel's counts written in another order", ST,
  "        channel.answered,\n        channel.peer,\n",
  "        channel.peer,\n        channel.answered,\n", "bd"),
 ("S2 a v17 file read with a channel", ST,
  "let channel = (version >= 18).then(", "let channel = (version >= 17).then(", "bd"),
 ("S3 a v17 file read with the publishes", ST,
  "publish: (version >= 18).then(|| PublishShown {", "publish: (version >= 17).then(|| PublishShown {", "bd"),
 # The author's answer (A1 to A11), as described.
 ("A1 a store call before the answer, between two turns", UL,
  "                        let now = self.control(control);\n",
  "                        tokio::time::sleep(Duration::from_secs(21)).await;\n                        let now = self.control(control);\n", "u"),
 ("A2/A2d the count given only with one of the uploader's calls", QU,
  "        let counts = std::mem::take(&mut self.slot().counts);\n",
  "        let counts = if self.slot().call.is_some() { std::mem::take(&mut self.slot().counts) } else { Vec::new() };\n", "ud"),
 ("A3/A3d a closed desk answers a count of nothing", QU,
  "            if slot.closed {\n                return None;\n            }\n            // Asks whose askers",
  "            if slot.closed {\n                return Some(CutsNow::default());\n            }\n            // Asks whose askers", "ud"),
 ("A4 the handler answers a closed desk's None as a count of nothing", HD,
  "        Ok(None) => Err(ended()),", "        Ok(None) => Ok(skyzfs_upload::CutsNow::default()),", "d"),
 ("A5 the count waited for with no limit", HD,
  ".block_on(async move { tokio::time::timeout(limit, desk.cuts_now()).await })",
  ".block_on(async move { let _ = limit; Ok::<_, tokio::time::error::Elapsed>(desk.cuts_now().await) })", "d"),
 ("A6 the wake's connect blocks", SV,
  "        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,\n        None,\n    ) else {",
  "        SocketFlags::CLOEXEC,\n        None,\n    ) else {", "d"),
 ("A7 each read given IO_LIMIT, not the time left", SV,
  "self.stream.set_read_timeout(Some(left))?;", "let _ = left; self.stream.set_read_timeout(Some(IO_LIMIT))?;", "d"),
 ("A8 a name the uploader would refuse is taken at parse", CT,
  "if name.is_some_and(|name| !valid_name(name)) {", "if false && name.is_some_and(|name| !valid_name(name)) {", "d"),
 ("A9 every user may ask", SV,
  "uid == 0 || uid == rustix::process::geteuid().as_raw()",
  "uid == 0 || uid == rustix::process::geteuid().as_raw() || uid != 0", "d"),
 ("A10 another holder's reason dropped", SE,
  "Some(Some(AtStart::Another { holder, epoch })) => format!(",
  "Some(Some(AtStart::Another { holder, epoch })) if false => format!(", "d"),
 ("A11 the state given at the start not kept", PB,
  "            attachment,\n            state,\n            stopped: false,\n",
  "            attachment,\n            state: { let _ = state; None },\n            stopped: false,\n", "u"),
 # Mine from the first look, still applicable.
 ("M2 the socket keeps the umask's mode", SV,
  "fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;",
  "let _ = fs::Permissions::from_mode(0o600);", "d"),
 ("M3 a line of exactly LINE_MAX refused", CT, ".take(LINE_MAX as u64)", ".take(LINE_MAX as u64 - 1)", "d"),
 ("M4 a full inbox answered ended", HD,
  "mpsc::error::TrySendError::Full(_) => Reply::Refused(",
  "mpsc::error::TrySendError::Full(_) => ended(), mpsc::error::TrySendError::Full(_) if false => Reply::Refused(", "d"),
 ("M6 version refusals not counted malformed", SV,
  "Reply::Refused(Code::Malformed | Code::Version, _)", "Reply::Refused(Code::Malformed, _)", "d"),
 ("M7 a refused publish exits 0", "bins/skyzfsd/src/commands.rs",
  "exit: if reply.refused() {", "exit: if false && reply.refused() {", "d"),
 ("M8 the newest cut always opens as itself", HD,
  "newest: now.newest.map(|cut| (cut.group, cut.itself)),", "newest: now.newest.map(|cut| (cut.group, true)),", "d"),
 ("M9 take_ended does not drain", PB, "self.ended.drain(..).collect()", "self.ended.iter().cloned().collect()", "u"),
 ("M10 the attachment not told to the uploader", UP,
  "uploader.publishing().set_attachment(attachment);", "let _ = attachment;", "d"),
 ("M11 a peer refusal not written", SV,
  "let _ = stream.write_all(format!(\"{HELLO}\\n{}\\n\", refusal.line()).as_bytes());", "let _ = &refusal;", "d"),
 ("M12 an asked line with G and C swapped", RE,
  "(\"asked\", group.to_string(), after.to_string())", "(\"asked\", after.to_string(), group.to_string())", "d"),
 ("M14 the early expiry refusal dropped", CT,
  "if p.expiry.is_some_and(|e| e.is_zero() || e > EXPIRY_MAX) {",
  "if false && p.expiry.is_some_and(|e| e.is_zero() || e > EXPIRY_MAX) {", "d"),
 # New, on the new code.
 ("N1 a held sync withholds the count", QU,
  "        let counts = std::mem::take(&mut self.slot().counts);\n",
  "        let counts = if self.slot().syncing.is_some() { Vec::new() } else { std::mem::take(&mut self.slot().counts) };\n", "ud"),
 ("N2 an ask of the count does not wake the queue", QU,
  "            slot.counts.push(reply);\n        }\n        (self.wake)();\n",
  "            slot.counts.push(reply);\n        }\n", "ud"),
 ("N3 the close leaves the asks of the count waiting", QU,
  "        slot.counts.clear();\n", "", "ud"),
 ("N4 the counts counted as calls", QU,
  "                let _ = reply.send(now);\n",
  "                self.slot().answered += 1;\n                let _ = reply.send(now);\n", "ud"),
 ("N5 every accept error retried at once", SV,
  "fn gave_up(e: &io::Error) -> bool {\n    matches!(", "fn gave_up(e: &io::Error) -> bool {\n    true || matches!(", "d"),
 ("N6 any request begins the next turn at once", UL,
  "                        self.post(&mut publish);\n                        if now {\n",
  "                        self.post(&mut publish);\n                        if now || true {\n", "u"),
 ("N7 a deadline 100 times as long", SV,
  "        let until = Instant::now() + request_limit;\n",
  "        let until = Instant::now() + request_limit * 100;\n", "d"),
 ("N8 a name checked only for publish", CT,
  "        Request::Progress(name) | Request::Withdraw(name) => Some(name),\n",
  "        Request::Progress(_) | Request::Withdraw(_) => None,\n", "d"),
]
only = sys.argv[1:]
res = []
for name, f, old, new, suites in P:
    if only and not any(name.startswith(o) for o in only):
        continue
    path = os.path.join(W, f)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        res.append((name, f"NOT APPLIED (found {n})"))
        print(f"{name}: NOT APPLIED ({n})", flush=True)
        continue
    t = time.time()
    verdict = "SURVIVED"
    try:
        open(path, "w").write(text.replace(old, new))
        for s in suites:
            try:
                r = subprocess.run(SUITES[s], cwd=W, env=ENV, capture_output=True, text=True, timeout=900)
            except subprocess.TimeoutExpired:
                verdict = f"caught ({s}: timeout, a hang)"
                break
            out = r.stdout + r.stderr
            if "error[E" in out or "could not compile" in out:
                verdict = "BUILD ERROR"
                print(out[-3000:])
                break
            if r.returncode != 0:
                failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
                verdict = f"caught ({s}): " + ", ".join(failed[:4]) + (f" and {len(failed) - 4} more" if len(failed) > 4 else "")
                break
    finally:
        open(path, "w").write(text)
    res.append((name, verdict))
    print(f"{name}: {verdict} ({time.time() - t:.0f}s)", flush=True)
json.dump(res, open("plants-%d.json" % os.getpid(), "w"), indent=1)
print("done")
