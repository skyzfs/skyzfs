#!/usr/bin/env python3
"""Build the upload crate's driver test binary at the head and with B1H
planted (textual, restored in a finally), and copy each to BINDIR.
Usage: l1_build.py TREE BINDIR"""
import json, os, shutil, subprocess, sys
TREE, BINDIR = sys.argv[1], sys.argv[2]
REMOTE = os.path.join(TREE, 'crates/skyzfs-upload/src/remote.rs')
OLD = '            head_group: engine.head_group(),\n'
NEW = '            head_group: engine.commit_group(&head.commit).await?,\n'
ENV = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0')


def build(name):
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-upload', '--test', 'driver', '--no-run', '--message-format=json'],
                       cwd=TREE, capture_output=True, text=True, timeout=3600, env=ENV)
    exe = [json.loads(l).get('executable') for l in p.stdout.splitlines() if l.startswith('{')]
    exe = [e for e in exe if e]
    assert p.returncode == 0 and exe, p.stderr[-3000:]
    shutil.copy2(exe[-1], os.path.join(BINDIR, name))
    print(name, 'from', os.path.basename(exe[-1]), flush=True)


build('driver-head')
orig = open(REMOTE).read()
try:
    assert orig.count(OLD) == 1
    open(REMOTE, 'w').write(orig.replace(OLD, NEW))
    build('driver-B1H')
finally:
    open(REMOTE, 'w').write(orig)
