#!/usr/bin/env python3
"""Defects planted at fcec1e4 (pull request 64, M1.3aa), one at a time,
and which tests of skyzfs-ublk, skyzfs-ublk-sys and skyzfsd (with its
test hooks) fail. Each file is put back after its plant.

    plants64.py WORKTREE TARGET_DIR [name...]
"""
import os
import re
import subprocess
import sys
import time

W, TARGET = sys.argv[1], sys.argv[2]
QUEUE = 'crates/skyzfs-ublk-sys/src/queue.rs'
PUMP = 'crates/skyzfs-ublk/src/pump.rs'
STATUS = 'crates/skyzfs-ublk/src/status.rs'
RECORD = 'bins/skyzfsd/src/record.rs'
DELAY = 'bins/skyzfsd/src/delay.rs'
COUNT = '        self.count_unfetched(&unfetched);\n'

# name: (file, [(old, new, which occurrence or None for the only one)])
PLANTS = {
    'the kernel queue drops what the driver ended otherwise than OK': (QUEUE, [
        ('Some(Err(command)) => unfetched.push(command),', 'Some(Err(_)) => {}', None)]),
    'fetch and commit named for each other': (QUEUE, [
        ('FETCH => Command::Fetch,\n        COMMIT => Command::Commit,',
         'FETCH => Command::Commit,\n        COMMIT => Command::Fetch,', None)]),
    'bit 62, not 63, marks what is no command': (QUEUE, [
        ('    if user_data & (1 << 63) != 0 {\n        return None;', '    if user_data & (1 << 62) != 0 {\n        return None;', None)]),
    'a positive result fetched a request': (QUEUE, [
        ('if result == sys::UBLK_IO_RES_OK as i32 {', 'if result >= 0 {', None)]),
    'an abort is counted as a failure': (PUMP, [
        ('if command.result == ABORTED {', 'if command.result == ABORTED && false {', None)]),
    'the first failure is kept, not the last': (PUMP, [
        ('                self.counters.last_command_failure = command.result;',
         '                if self.counters.last_command_failure == 0 { self.counters.last_command_failure = command.result; }', None)]),
    'the turn of a failed volume does not count': (PUMP, [(COUNT, '', 0)]),
    'the normal turn does not count': (PUMP, [(COUNT, '', 1)]),
    'aborted and failed change places in the file': (STATUS, [
        ('bytes[NINTH_LEN..NINTH_LEN + 8].copy_from_slice(&c.commands_aborted.to_le_bytes());',
         'bytes[NINTH_LEN..NINTH_LEN + 8].copy_from_slice(&c.commands_failed.to_le_bytes());', None),
        ('bytes[NINTH_LEN + 8..NINTH_LEN + 16].copy_from_slice(&c.commands_failed.to_le_bytes());',
         'bytes[NINTH_LEN + 8..NINTH_LEN + 16].copy_from_slice(&c.commands_aborted.to_le_bytes());', None),
        ('commands_aborted: if tenth { u64_at(bytes, 2008) } else { 0 },', 'commands_aborted: if tenth { u64_at(bytes, 2016) } else { 0 },', None),
        ('commands_failed: if tenth { u64_at(bytes, 2016) } else { 0 },', 'commands_failed: if tenth { u64_at(bytes, 2008) } else { 0 },', None)]),
    'a file of the ninth version is refused': (STATUS, [
        ('let ninth = tenth || (bytes.len() == NINTH_LEN && &bytes[..8] == NINTH_MAGIC);', 'let ninth = tenth;', None)]),
    'the report shows the counts for an older file': (STATUS, [
        ('        if s.version >= 10 {\n            lines.push(("commands_aborted"', '        if s.version >= 1 {\n            lines.push(("commands_aborted"', None)]),
    'the last failure is shown before any': (STATUS, [
        ('            if c.commands_failed > 0 {', '            if c.commands_failed < u64::MAX {', None)]),
    'the last failure loses its sign when read': (STATUS, [
        ('i32::from_le_bytes([bytes[2024], bytes[2025], bytes[2026], bytes[2027]])',
         'i32::from_le_bytes([bytes[2024], bytes[2025], bytes[2026], bytes[2027]]).wrapping_abs()', None)]),
    'no ended line when the wait fails': (RECORD, [
        ('in unfetched.get(ended_before..).unwrap_or_default().to_vec()',
         'in unfetched.get(ended_before..).filter(|_| woken.is_ok()).unwrap_or_default().to_vec()', None)]),
    'earlier commands get their line again': (RECORD, [
        ('in unfetched.get(ended_before..).unwrap_or_default().to_vec()',
         'in unfetched.get(0..).unwrap_or_default().to_vec()', None)]),
    'the delay wrapper drops them': (DELAY, [
        ('let woken = self.inner.wait(fetched, unfetched)?;', 'let woken = self.inner.wait(fetched, &mut Vec::new())?;', None)]),
}


def run():
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
    t = time.time()
    try:
        p = subprocess.run(['cargo', 'test', '-q', '--no-fail-fast', '-p', 'skyzfs-ublk', '-p', 'skyzfs-ublk-sys',
                            '-p', 'skyzfsd', '--features', 'skyzfsd/test-hooks'], cwd=W, env=env,
                           capture_output=True, text=True, timeout=1800)
    except subprocess.TimeoutExpired:
        return 'HUNG (1800 s)'
    out = p.stdout + p.stderr
    if 'error[' in out or 'could not compile' in out:
        errs = [l for l in out.splitlines() if l.startswith('error')][:3]
        return 'DOES NOT BUILD: ' + ' | '.join(errs)
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)) | set(re.findall(r'^    (\S+::\S+)$', out, re.M)))
    passed = sum(int(n) for n in re.findall(r'test result: \w+\. (\d+) passed', out))
    return (('FAILS: ' + ', '.join(failed)) if failed else 'nothing fails') + f' ({passed} passed, {time.time() - t:.0f} s)'


def plant(name):
    path, edits = PLANTS[name]
    full = os.path.join(W, path)
    orig = open(full).read()
    text = orig
    for old, new, which in edits:
        n = text.count(old)
        if which is None:
            assert n == 1, (name, old, n)
            text = text.replace(old, new)
        else:
            assert n > which, (name, old, n)
            parts = text.split(old)
            text = old.join(parts[:which + 1]) + new + old.join(parts[which + 1:])
    open(full, 'w').write(text)
    try:
        return run()
    finally:
        open(full, 'w').write(orig)


names = sys.argv[3:] or ['unchanged'] + list(PLANTS)
for name in names:
    print(f'{name}: {run() if name == "unchanged" else plant(name)}', flush=True)
