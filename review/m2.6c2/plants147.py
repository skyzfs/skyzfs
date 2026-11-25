#!/usr/bin/env python3
"""Plants for the review of #147 (M2.6c-2).

Run from the root of a worktree at the head under review, with the two
scratch modules planted (scratch147_host.rs as
crates/skyzfs-agent/src/host/scratch147.rs, scratch147_client.rs as
crates/skyzfs-control/src/scratch147.rs, each declared `#[cfg(test)] mod
scratch147;`). For each plant: apply it alone, run the crate's own tests
(`--skip scratch147`), then the scratch tests named for it, and restore
the file to what it was before the plant.

    python3 plants147.py [ID...] > plants147.out

A plant "survives" a set of tests when they all pass with it.
"""

import json
import os
import subprocess
import sys
import time

AGENT = "skyzfs-agent"
CONTROL = "skyzfs-control"
HOST = "crates/skyzfs-agent/src/host.rs"
CLI = "crates/skyzfs-agent/src/zfs/cli.rs"
DAEMON = "crates/skyzfs-agent/src/daemon.rs"
CLIENT = "crates/skyzfs-control/src/client.rs"
SNAP_LUA = "crates/skyzfs-agent/src/zfs/cli/snapshot.lua"
MEMBERS_LUA = "crates/skyzfs-agent/src/zfs/cli/members.lua"

# id, who, file, old, new, crate, scratch filter (or None), what
PLANTS = [
    # A sample of the author's plants, rebuilt from their descriptions.
    ("C1", "author", CLIENT,
     "        let left = self.left()?;\n        self.stream.set_read_timeout(Some(left))?;",
     "        let left = Duration::from_millis(300);\n        self.stream.set_read_timeout(Some(left))?;",
     CONTROL, None, "a timeout per read (S4 undone for reads)"),
    ("H1", "author", HOST,
     "                abandon(child);\n                return Err(",
     "                abandon(child);\n                let _ = stdout.recv();\n                return Err(",
     AGENT, None, "output waited for after the kill"),
    ("H2", "author", HOST,
     "            if left.is_zero() {",
     "            if left.is_zero() && program.is_empty() {",
     AGENT, None, "no limit"),
    ("H3", "author", HOST,
     "            match pipe.recv_timeout(left.max(Duration::from_millis(1))) {",
     "            match pipe.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) {",
     AGENT, None, "output read with no limit"),
    ("Z8", "author", CLI,
     '            if errno.parse::<i64>() == Ok(EEXIST) && !returned.contains_key("taken") {',
     '            if errno.parse::<i64>() == Ok(EEXIST) {',
     AGENT, None, "EEXIST after a partial take read as Exists"),
    ("Z9", "author", CLI,
     '            Err(ZfsError::Other(why)) if why.contains("no such tag") => {\n',
     '            Err(ZfsError::Other(why)) if why.contains("no such tag") => {\n'
     '                if !why.is_empty() {\n                    return Ok(());\n                }\n',
     AGENT, None, "no such tag released with no look"),
    ("Z12", "author", CLI,
     '            Err(ZfsError::Other(why)) if why.contains("tag already exists") => Ok(()),\n',
     '',
     AGENT, None, "tag already exists an error"),
    ("Z19", "author", CLI,
     "self.program(pool, SNAPSHOT_PROGRAM, &[pool, t, ASSIST_PROPERTY], false)",
     "self.program(pool, SNAPSHOT_PROGRAM, &[pool, t, ASSIST_PROPERTY], true)",
     AGENT, None, "snapshot program run read-only"),
    ("D1", "author", DAEMON,
     "            Node::File | Node::Other => Err(io::Error::new(",
     "            Node::File | Node::Other => Ok(0),\n            #[allow(unreachable_patterns)]\n            _ => Err(io::Error::new(",
     AGENT, None, "any node a device"),
    # The review's own.
    ("R1", "review", HOST,
     "                abandon(child);\n",
     "                drop(child);\n",
     AGENT, "host::scratch147::r1", "the child dropped, not killed or reaped"),
    ("R2", "review", HOST,
     "    thread::spawn(move || {\n        let _ = child.wait();\n    });\n",
     "    drop(child);\n",
     AGENT, "host::scratch147::r", "killed, never reaped (a zombie per call)"),
    ("R3", "review", CLIENT,
     "    let until = Instant::now() + limit;\n    let stream = connect(path, limit)?;\n",
     "    let stream = connect(path, limit)?;\n    let until = Instant::now() + limit;\n",
     CONTROL, "scratch147::k5", "the deadline starts after the connect"),
    ("R4", "review", CLI,
     "(fields.next() == Some(tag)).then_some(name)",
     "(fields.nth(1) == Some(tag)).then_some(name)",
     AGENT, None, "release's look reads the wrong field: nobody keeps the tag"),
    ("R5", "review", CLI,
     "    if out.first().map(|m| m.name.as_str()) != Some(pool) {\n        return Err(other(",
     "    if out.first().is_none() {\n        return Err(other(",
     AGENT, None, "snapshot program: the root not required"),
    ("R6", "review", CLI,
     "    if out.first().map(|m| m.name.as_str()) != Some(pool) {\n        return Err(format!(",
     "    if out.first().is_none() {\n        return Err(format!(",
     AGENT, None, "members program: the root not required"),
    ("R7", "review", CLI,
     "                fields.next() == Some(snapshot.as_str()) && fields.next() == Some(tag)",
     "                fields.next().is_some() && fields.next() == Some(tag)",
     AGENT, None, "held: the snapshot's name not compared"),
    ("R8", "review", CLI,
     "                        Err(ZfsError::NoSnapshot) => return Ok(()),",
     "                        Err(ZfsError::NoSnapshot) => return Err(ZfsError::NoSnapshot),",
     AGENT, None, "release: holds -r with no snapshot an error"),
    ("R9", "review", DAEMON,
     "skyzfs_control::ask(&self.socket, request, manifest, self.limit)",
     "skyzfs_control::ask(&self.socket, request, manifest, skyzfs_control::ASK_LIMIT)",
     AGENT, None, "DaemonSocket's limit ignored"),
    ("R10", "review", SNAP_LUA,
     "        if source == ds and value ~= nil then",
     "        if value ~= nil then",
     AGENT, None, "snapshot.lua: inherited and default system properties taken as local"),
    ("R11", "review", SNAP_LUA,
     "        if source == ds and property ~= assist then",
     "        if property ~= assist then",
     AGENT, None, "snapshot.lua: inherited user properties taken as local"),
    ("R12", "review", MEMBERS_LUA,
     'if not zfs.exists(pool .. "@" .. t) then\n    return {absent = "1"}\nend\n',
     '',
     AGENT, None, "members.lua: an absent pool@t never said"),
    ("R13", "review", CLI,
     '    if snapshot && message.contains("dataset does not exist") {',
     '    if message.contains("dataset does not exist") {',
     AGENT, None, "dataset does not exist read as NoSnapshot for every command"),
    ("R14", "review", CLIENT,
     "        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => timed_out(),",
     "        io::ErrorKind::TimedOut => timed_out(),",
     CONTROL, "scratch147::k", "a socket's timeout given as WouldBlock"),
    ("R15", "review", HOST,
     '            .env("LC_ALL", "C")\n',
     '            .env("LC_ALL", "C.UTF-8")\n',
     AGENT, None, "the locale C.UTF-8 (what F2's other fix would do)"),
]

ENV = dict(os.environ)


def run(args, timeout):
    began = time.time()
    try:
        p = subprocess.run(args, capture_output=True, text=True, timeout=timeout, env=ENV)
        out = p.stdout + p.stderr
        code = p.returncode
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"").decode() if isinstance(e.stdout, bytes) else (e.stdout or "")
        code = "timeout"
    return code, out, time.time() - began


def failed_tests(out):
    names = []
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("test ") and line.endswith("FAILED"):
            names.append(line[5:].rsplit(" ...", 1)[0])
    return names


def summary(out):
    for line in out.splitlines():
        if line.startswith("test result:"):
            return line
    if "error[" in out or "could not compile" in out:
        return "did not build"
    return "no result"


def main():
    only = set(sys.argv[1:])
    rows = []
    for pid, who, path, old, new, crate, scratch, what in PLANTS:
        if only and pid not in only:
            continue
        with open(path) as f:
            before = f.read()
        if before.count(old) != 1:
            rows.append({"id": pid, "who": who, "what": what,
                         "error": f"the plant's text occurs {before.count(old)} times"})
            print(json.dumps(rows[-1]), flush=True)
            continue
        with open(path, "w") as f:
            f.write(before.replace(old, new))
        try:
            code, out, took = run(["cargo", "test", "-q", "-p", crate, "--lib", "--",
                                   "--skip", "scratch147"], 900)
            row = {"id": pid, "who": who, "what": what, "crate": crate,
                   "own_tests": "survives" if code == 0 else "caught",
                   "own_result": summary(out), "own_failed": failed_tests(out),
                   "own_seconds": round(took)}
            if scratch:
                code2, out2, took2 = run(["cargo", "test", "-q", "-p", crate, "--lib", scratch,
                                          "--", "--test-threads=1"], 900)
                row.update({"scratch": scratch,
                            "scratch_tests": "survives" if code2 == 0 else "caught",
                            "scratch_result": summary(out2),
                            "scratch_failed": failed_tests(out2)})
        finally:
            with open(path, "w") as f:
                f.write(before)
        rows.append(row)
        print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
