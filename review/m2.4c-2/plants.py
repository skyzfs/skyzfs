#!/usr/bin/env python3
"""Plants for the review of #118 at e42ea6a (M2.4c-2).

    plants.py WORKTREE TARGET_DIR [ID ...]

One change at a time is written into a scratch worktree of the head, the
pull request's own tests are run, and the file is restored. A plant is

- KILLED       when a test of the pull request fails with it,
- SURVIVED     when every test passes with it,
- NOT APPLIED  when its text is not found exactly once, or it does not
               build.

For a plant that survives and is marked `scen`, the review's scenarios
(scenarios.rs) are run as well: "scenarios: KILLED" means the set of
failing scenarios differs from the head's (the three `defect_` scenarios).

Each result is appended to plants.out as it comes; a plant already there
is skipped on a rerun. The worktree must be clean at the start and is
checked clean at the end.
"""
import os
import re
import signal
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "plants.out")

SERVE = "bins/skyzfsd/src/serve.rs"
UPLOAD = "bins/skyzfsd/src/upload.rs"
KILL = "bins/skyzfsd/src/kill.rs"
CLI = "bins/skyzfsd/src/cli.rs"
COMMANDS = "bins/skyzfsd/src/commands.rs"
CUTS = "bins/skyzfsd/src/cut_commands.rs"
REMOTE = "bins/skyzfsd/src/remote.rs"
LIFE = "crates/skyzfs-ublk/src/lifecycle.rs"
STATUS = "crates/skyzfs-ublk/src/status.rs"

# The pull request's tests. D: the daemon's, in the build with the test
# hooks (a superset of the default build's). P: the daemon's, default
# build. U: the transport's.
D = ["cargo", "test", "-p", "skyzfsd", "--features", "skyzfsd/test-hooks", "--no-fail-fast"]
P = ["cargo", "test", "-p", "skyzfsd", "--no-fail-fast"]
U = ["cargo", "test", "-p", "skyzfs-ublk", "--lib", "--no-fail-fast"]
SCEN = ["cargo", "test", "-p", "skyzfsd", "--test", "review_m24c2", "--no-fail-fast",
        "--", "--test-threads=1"]
LIMIT = 300
BASELINE = {"defect_s1_create_takes_a_clone_that_holds_data_as_empty",
            "defect_s2_the_status_names_no_holder_while_the_lease_is_held",
            "defect_d7_the_end_waits_beyond_its_limit_for_the_runtimes_blocking_work"}

ANSWER = """                    if ended {
                        desk.close();
                    } else {
                        desk.answer(pump.target());
                    }
"""
LEASED = """        let (open, size) = self.inner.open(lock, acknowledge_failure)?;
        match (self.take)() {
            Ok(()) => Ok((open, size)),
            Err(why) => {
                let lock = (self.share)(&open);
                drop(open);
                Err((OpenError::Refused(why), lock))
            }
        }
"""
HEAD_CHECK = "    if commit.parent.is_some() || !commit.segments.is_empty() || commit.group.is_some() {\n"
PUT = """        self.inner.put(key, data).await?;
        if key.contains("/seg/") {
            let n = self.puts.fetch_add(1, Ordering::SeqCst) + 1;
            if self.at == Some(n) {
                (self.die)(&format!("after segment PUT {n} landed ({key})"));
            }
        }
        Ok(())
"""
SWAP = """        self.inner.commit(group, expected, row).await?;
        self.swaps += 1;
        if self.at == Some(self.swaps) {
            (self.die)(&format!(
                "after the swap of commit {} (group {group}) landed, before its cut was let go",
                self.swaps
            ));
        }
        Ok(())
"""

# id, file, old, new, what, commands, scen
PLANTS = [
    # ---- serve.rs: the wiring in `run`
    ("W01", SERVE, ANSWER,
     "                    if ended {\n                        desk.close();\n                    }\n",
     "the queue's thread never answers the desk", [D], False),
    ("W02", SERVE, ANSWER,
     "                    if !ended {\n                        desk.answer(pump.target());\n                    }\n",
     "the queue's thread does not close the desk at its end", [D], False),
    ("W03", SERVE, "Desk::new(move || wake.wake())",
     "Desk::new(move || {\n            let _ = &wake;\n        })",
     "the desk's wake does nothing", [D], False),
    ("W04", SERVE, "let release = exit == Exit::Done;", "let release = true;",
     "the lease is let go at every exit that reaches the end", [D], False),
    ("W05", SERVE, "let release = exit == Exit::Done;", "let release = false;",
     "the lease is never let go", [D], False),
    ("W06", SERVE, "(serve.hold_cuts || uploads).then(|| HoldCuts {",
     "serve.hold_cuts.then(|| HoldCuts {",
     "a volume with a place does not hold its cuts unless --hold-cuts", [D], False),
    ("W07", SERVE, "volume.lock + volume.position + volume.recovery + lease_time",
     "volume.lock + volume.position + volume.recovery",
     "the lease's time is counted in the start's `devices` time", [D], False),
    ("W08", SERVE, "            return Ok(Exit::LeaseHeld);", "            return Ok(Exit::RefusedHeld);",
     "a start refused for the lease exits 8, not 10", [D], False),
    ("W09", SERVE,
     "        if serve.holder.is_some() || serve.lease_ttl_s.is_some() || serve.serve_without_upload {",
     "        if false {",
     "the lease's options are taken for a volume with no place", [D], False),
    ("W10", SERVE, "        if serve.hooks.kill_at.is_some() {", "        if false {",
     "--kill-at-upload is taken for a volume with no place", [D], False),
    ("W11", SERVE,
     "    let mut uploads = uploads.filter(|u| u.started().found().is_some_and(|(f, _)| f.uploads()));",
     "    let mut uploads = uploads;",
     "the uploader's thread starts under another holder's lease (--serve-without-upload)", [D], False),
    ("W12", SERVE, "        .uploading(uploads.is_some());", "        .uploading(false);",
     "the hook command `release` is taken in a daemon that uploads", [D], False),
    ("W13", SERVE, "        .unwrap_or(crate::upload::DEFAULT_LEASE_TTL_S);", "        .unwrap_or(3600);",
     "the lease's default time is an hour", [D], False),
    # ---- upload.rs: the start
    ("U01", UPLOAD, LEASED,
     "        if let Err(why) = (self.take)() {\n            return Err((OpenError::Refused(why), lock));\n        }\n"
     "        self.inner.open(lock, acknowledge_failure)\n",
     "the lease is taken before the open", [D], True),
    ("U02", UPLOAD, "                drop(open);\n                Err((OpenError::Refused(why), lock))",
     "                std::mem::forget(open);\n                Err((OpenError::Refused(why), lock))",
     "a refusal leaves the store open", [D], True),
    ("U03", UPLOAD, "Ok(LeaseState::Lost { holder, epoch }) if !self.serve_without_upload => {",
     "Ok(LeaseState::Lost { holder, epoch }) if false => {",
     "another holder's lease never refuses the start", [D], True),
    ("U04", UPLOAD, "Ok(LeaseState::Lost { holder, epoch }) if !self.serve_without_upload => {",
     "Ok(LeaseState::Lost { holder, epoch }) if true => {",
     "--serve-without-upload is not heeded", [D], False),
    ("U05", UPLOAD,
     "            Err(_) => AtStart::Unreached(format!(\n"
     "                \"object storage did not answer within {} ms\",\n"
     "                limit.as_millis()\n            )),\n",
     "            Err(_) => return Err(\"object storage did not answer\".into()),\n",
     "a take with no answer within the limit refuses the start", [D], True),
    ("U06", UPLOAD,
     "            Ok(LeaseState::None(why)) => {\n"
     "                AtStart::Unreached(format!(\"object storage could not be reached: {why}\"))\n"
     "            }\n",
     "            Ok(LeaseState::None(why)) => return Err(why),\n",
     "object storage out of reach refuses the start", [D], False),
    ("U07", UPLOAD, "        !matches!(self, AtStart::Another { .. })", "        true",
     "AtStart::uploads says yes under another holder's lease", [D], False),
    # ---- upload.rs: the thread's end
    ("U08", UPLOAD,
     "                    if !release {\n                        return Ended::Kept;\n                    }\n",
     "",
     "the uploader's thread lets the lease go at every end", [D], True),
    ("U09", UPLOAD, "let release = stopped.borrow().unwrap_or(false);", "let release = false;",
     "the uploader's thread never lets the lease go", [D], True),
    ("U10", UPLOAD, "        self.desk.close();\n        let _ = self.stop.send(Some(release));",
     "        let _ = self.stop.send(Some(release));",
     "ending the thread does not close the desk", [D], True),
    ("U11", UPLOAD, "        self.finish(false, END_LIMIT);", "        self.finish(true, END_LIMIT);",
     "a drop of the thread's handle (every exit but 0) lets the lease go", [D], True),
    ("U12", UPLOAD, "                        Ok(Ok(())) if held => Ended::Released,",
     "                        Ok(Ok(())) => Ended::Released,",
     "an end with no lease held says it was let go", [D], False),
    # ---- upload.rs: create's place
    ("U13", UPLOAD, "    if let Some(lease) = lease {\n        return Err(format!(\n            \"{named} has been served",
     "    if let Some(lease) = lease.filter(|_| false) {\n        return Err(format!(\n            \"{named} has been served",
     "create takes a volume that has a lease row", [D], False),
    ("U14", UPLOAD, "    if superblock.size != size {", "    if false {",
     "create takes a volume of another size", [D], False),
    ("U15", UPLOAD, "    if superblock.shard_bytes != SHARD_BYTES {", "    if false {",
     "create takes a volume with other shards", [D], False),
    ("U16", UPLOAD, HEAD_CHECK,
     "    if !commit.segments.is_empty() || commit.group.is_some() {\n",
     "create takes a head commit that has a parent", [D], False),
    ("U17", UPLOAD, HEAD_CHECK,
     "    if commit.parent.is_some() || commit.group.is_some() {\n",
     "create takes a head commit that has segments", [D], False),
    ("U18", UPLOAD, HEAD_CHECK,
     "    if commit.parent.is_some() || !commit.segments.is_empty() {\n",
     "create takes a head commit that names a group", [D], False),
    ("U19", UPLOAD, "        shard_bytes: SHARD_BYTES,\n        epoch,",
     "        shard_bytes: SHARD_BYTES / 2,\n        epoch,",
     "the volume is made with shards of 64 MiB", [D], False),
    # ---- upload.rs: the remote and the status
    ("U20", UPLOAD, "ActingRefStore::new(refs.clone(), holder.clone(), epoch);",
     "ActingRefStore::new(refs.clone(), holder.clone(), epoch + 1);",
     "the uploader's commits are fenced by another epoch than the one held", [D], True),
    ("U20p", UPLOAD, "ActingRefStore::new(refs.clone(), holder.clone(), epoch);",
     "ActingRefStore::new(refs.clone(), holder.clone(), epoch + 1);",
     "the same as U20, against the default build's tests", [P], True),
    ("U21", UPLOAD, "EngineRemote::new(objects.clone(), Arc::new(acting), vol, engine_config(epoch))",
     "EngineRemote::new(objects.clone(), Arc::new(acting), vol, engine_config(0))",
     "the uploader's commits carry epoch 0", [D], True),
    ("U22", UPLOAD, "            out.lease = LeaseShown::Unsure;", "            out.lease = LeaseShown::Held;",
     "the status shows an unsure lease as held", [D], False),
    ("U23", UPLOAD,
     "            out.lease = LeaseShown::Lost;\n            out.epoch = *epoch;\n            out.holder = Text::new(holder);",
     "            out.lease = LeaseShown::Lost;\n            out.epoch = *epoch;\n            let _ = holder;",
     "the status names no holder for a lost lease", [D], False),
    ("U24", UPLOAD, "            out.epoch = *epoch;\n            out.expires_unix_ms = *expires_at_ms;",
     "            out.epoch = *epoch;\n            let _ = expires_at_ms;",
     "the status does not show when the lease runs out", [D], False),
    ("U25", UPLOAD,
     "        let make = crate::kill::maker(objects, refs, holder, vol, kill, crate::kill::sigkill());",
     "        let make = {\n            let _ = kill;\n            maker(objects, refs, holder, vol)\n        };",
     "the build with the test hooks does not give the uploader its kill point", [D], False),
    # ---- lifecycle.rs
    ("L01", LIFE, "            Err((OpenError::Refused(why), _)) => return Err(Stop::Refused(why)),",
     "            Err((OpenError::Refused(why), _)) => return Err(Stop::Error(why)),",
     "a refused open ends the start as an error", [U, D], False),
    ("L02", LIFE, "            Err((OpenError::Refused(why), _)) => return Err(Stop::Refused(why)),",
     "            Err((OpenError::Refused(why), _)) => {\n                let _ = records.remove_failure();\n"
     "                return Err(Stop::Refused(why));\n            }",
     "a refused open removes the failure note", [U, D], False),
    # ---- status.rs
    ("T01", STATUS, "        shown.upload = posted.store.upload;\n", "",
     "a post of the store's counters wipes the upload's part", [U], False),
    # ---- kill.rs
    ("K01", KILL, "        if key.contains(\"/seg/\") {", "        if true {",
     "put:N counts every PUT, not the segments'", [D], False),
    ("K02", KILL, PUT,
     "        if key.contains(\"/seg/\") {\n"
     "            let n = self.puts.fetch_add(1, Ordering::SeqCst) + 1;\n"
     "            if self.at == Some(n) {\n"
     "                (self.die)(&format!(\"after segment PUT {n} landed ({key})\"));\n"
     "            }\n        }\n        self.inner.put(key, data).await\n",
     "put:N kills before the PUT is sent, and counts one that fails", [D], False),
    ("K03", KILL, SWAP,
     "        self.swaps += 1;\n        if self.at == Some(self.swaps) {\n            (self.die)(&format!(\n"
     "                \"after the swap of commit {} (group {group}) landed, before its cut was let go\",\n"
     "                self.swaps\n            ));\n        }\n"
     "        self.inner.commit(group, expected, row).await\n",
     "swap:N kills before the commit is sent, and counts one that fails", [D], False),
    ("K04", KILL, "            Some(KillAt::Put(n)) => Some(n),",
     "            Some(KillAt::Put(n) | KillAt::Swap(n)) => Some(n),",
     "swap:N also kills at the Nth segment PUT", [D], False),
    # ---- cli.rs
    ("C01", CLI, "    opt(\"--kill-at-upload\", true, true),", "    opt(\"--kill-at-upload\", true, false),",
     "a default build takes --kill-at-upload", [P, D], False),
    ("C02", CLI, "            Exit::LeaseHeld => 10,", "            Exit::LeaseHeld => 9,",
     "the exit status of a start refused for the lease is 9", [D], False),
    ("C03", CLI, ".filter(|&n| n >= 1)", ".filter(|_| true)",
     "--kill-at-upload put:0 is taken", [D], False),
    ("C04", CLI, "            if lease_ttl_s.is_some_and(|s| !ttls.contains(&s)) {", "            if false {",
     "--lease-ttl-s is not bounded", [D], False),
    ("C05", CLI,
     "        _ => Err(UsageError::Refused(\n            \"--endpoint, --bucket and --volume: all three or none\".into(),\n        )),",
     "        _ => Ok(None),",
     "two of --endpoint --bucket --volume make a volume with no place", [D], False),
    # ---- commands.rs, cut_commands.rs, remote.rs
    ("M01", COMMANDS, "            write(&create.dir, remote).map_err(|e| {",
     "            Ok::<(), io::Error>(()).map_err(|e| {",
     "create does not write the file `remote`", [D], False),
    ("M02", COMMANDS,
     "            Err(why) => {\n                return Output::error(format_args!(\n"
     "                    \"{shown}: not created: {remote}: {why}. Nothing was made in {shown}\"\n"
     "                ))\n            }\n",
     "            Err(_) => None,\n",
     "a place refused makes the local volume all the same, with no place", [D], False),
    ("X01", CUTS, "            Ok(Command::Release { .. }) if self.uploading => refused(",
     "            Ok(Command::Release { .. }) if false => refused(",
     "the hook command `release` is never refused", [D], False),
    ("R01", REMOTE, "        if id != remote.id.0.to_string() {", "        if false {",
     "the file's id is not checked against the name's", [D], False),
    ("R02", REMOTE, "        if lines.next().is_some() {\n            return Err(unreadable(\"a line after the id\"));",
     "        if false {\n            return Err(unreadable(\"a line after the id\"));",
     "a line after the id is taken", [D], False),
]


def status(tree):
    return subprocess.run(["git", "-C", tree, "status", "--short"],
                          capture_output=True, text=True).stdout.strip()


def run(cmd, tree, target):
    """(verdict, detail): 'pass', 'fail' with the failing tests, 'build', 'timeout'."""
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=target, RUST_BACKTRACE="0")
    # A plant can make a test of the head wait for ever: the run is ended,
    # with its test processes, after LIMIT seconds (a run takes about one
    # minute here).
    proc = subprocess.Popen(cmd, cwd=tree, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        text, _ = proc.communicate(timeout=LIMIT)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        text, _ = proc.communicate()
        running = sorted(set(re.findall(r"^test (\S+) has been running for over", text, re.M)))
        names = ", ".join(name.split("::")[-1] for name in running[:3])
        return "timeout", f"no end within {LIMIT} s" + (f"; still running: {names}" if names else "")
    done = proc
    if "could not compile" in text:
        errors = re.findall(r"^error(?:\[E\d+\])?: (.*)$", text, re.M)
        return "build", "; ".join(errors[:2])
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", text, re.M)))
    if done.returncode == 0 and not failed:
        return "pass", ""
    return "fail", failed


def scenarios(tree, target):
    there = os.path.join(tree, "bins/skyzfsd/tests/review_m24c2.rs")
    try:
        with open(os.path.join(HERE, "scenarios.rs")) as f, open(there, "w") as g:
            g.write(f.read())
        verdict, detail = run(SCEN, tree, target)
    finally:
        if os.path.exists(there):
            os.remove(there)
    if verdict == "build":
        return f"scenarios: do not build ({detail})"
    if verdict == "timeout":
        return f"scenarios: KILLED ({detail})"
    failed = set(detail) if verdict == "fail" else set()
    if failed == BASELINE:
        return "scenarios: SURVIVED"
    new = sorted(failed - BASELINE)
    gone = sorted(BASELINE - failed)
    said = ", ".join(n.split("_")[0] for n in new)
    if gone:
        said += (" " if said else "") + "(now passing: " + ", ".join(n.split("_")[1] for n in gone) + ")"
    return f"scenarios: KILLED by {said}"


def main():
    tree, target, only = sys.argv[1], sys.argv[2], set(sys.argv[3:])
    if status(tree):
        sys.exit("the worktree is not clean:\n" + status(tree))
    done = set()
    if os.path.exists(OUT):
        with open(OUT) as f:
            done = {line.split(" | ")[0] for line in f if " | " in line}
    for ident, path, old, new, what, cmds, scen in PLANTS:
        if ident in done or (only and ident not in only):
            continue
        full = os.path.join(tree, path)
        with open(full) as f:
            source = f.read()
        if source.count(old) != 1:
            result = f"NOT APPLIED (its text is found {source.count(old)} times)"
        else:
            try:
                with open(full, "w") as f:
                    f.write(source.replace(old, new))
                result = None
                for cmd in cmds:
                    verdict, detail = run(cmd, tree, target)
                    if verdict == "build":
                        result = f"NOT APPLIED (does not build: {detail})"
                        break
                    if verdict == "timeout":
                        result = f"KILLED: the head's tests have {detail}"
                        break
                    if verdict == "fail":
                        names = [name.split("::")[-1] for name in detail] or ["a run that failed and named no test"]
                        more = f" and {len(names) - 3} more" if len(names) > 3 else ""
                        result = f"KILLED by {', '.join(names[:3])}{more}"
                        break
                if result is None:
                    result = "SURVIVED"
                    if scen:
                        result += "; " + scenarios(tree, target)
            finally:
                with open(full, "w") as f:
                    f.write(source)
        line = f"{ident} | {path} | {what} | {result}\n"
        with open(OUT, "a") as f:
            f.write(line)
        print(line, end="", flush=True)
    left = status(tree)
    print("worktree after:", left or "clean")


if __name__ == "__main__":
    main()
