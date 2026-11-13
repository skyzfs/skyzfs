#!/usr/bin/env python3
"""Defects planted at 45b67f2 (pull request 69, M1.3t), one at a time, and
which tests fail: of skyzfs-local, and for the plants in WIDE also of
skyzfs-ublk and skyzfsd (with its test hooks). Each file is put back after its plant.

    plants69.py WORKTREE TARGET_DIR [name...]
"""
import os
import re
import subprocess
import sys
import time

W, TARGET = sys.argv[1], sys.argv[2]
START = 'crates/skyzfs-local/src/inplace/start.rs'
VOL = 'crates/skyzfs-local/src/volume.rs'
INP = 'crates/skyzfs-local/src/inplace.rs'
POOL = 'crates/skyzfs-local/src/inplace/pool.rs'
HAR = 'crates/skyzfs-local/src/inplace/harness.rs'
FS = 'crates/skyzfs-local/src/fs.rs'

# name -> list of (file, old, new)
PLANTS = {
    'P1 the volume file is renamed without a sync of its content': [(VOL,
        "        file.write_at(0, &self.encode())?;\n        file.sync()?;\n",
        "        file.write_at(0, &self.encode())?;\n")],
    'P2 the volume file is made without the directory sync': [(VOL,
        "        dir.rename(VOLUME_NEW_NAME, VOLUME_NAME)?;\n        dir.sync()\n",
        "        dir.rename(VOLUME_NEW_NAME, VOLUME_NAME)?;\n        Ok(())\n")],
    'P3 the volume file is written under its own name, with no rename': [(VOL,
        "        let mut file = dir.create(VOLUME_NEW_NAME)?;\n        file.write_at(0, &self.encode())?;\n        file.sync()?;\n        drop(file);\n        dir.rename(VOLUME_NEW_NAME, VOLUME_NAME)?;\n",
        "        let mut file = dir.create(VOLUME_NAME)?;\n        file.write_at(0, &self.encode())?;\n        file.sync()?;\n        drop(file);\n")],
    'P4 the volume file is made before the home image': [
        (START,
         "    let creating = io_error(\"creating the home image\");\n",
         "    volume\n        .create(dir)\n        .map_err(io_error(\"creating the volume file\"))?;\n    let creating = io_error(\"creating the home image\");\n"),
        (START,
         "    dir.sync().map_err(&creating)?;\n    volume\n        .create(dir)\n        .map_err(io_error(\"creating the volume file\"))?;\n    Ok(volume)",
         "    dir.sync().map_err(&creating)?;\n    Ok(volume)")],
    'P5 the home image is renamed without its sync': [(START,
        "    home.sync().map_err(&creating)?;\n    drop(home);",
        "    drop(home);")],
    'P6 no directory sync between the rename of the home image and the volume file': [(START,
        "    dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;\n    dir.sync().map_err(&creating)?;\n",
        "    dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;\n")],
    'P7 an open refused for its volume file writes the failure marker': [(START,
        "        let volume = Volume::read(dir)?;\n",
        "        let volume = match Volume::read(dir) {\n            Ok(volume) => volume,\n            Err(e) => {\n                write_marker(dir, false);\n                return Err(e.into());\n            }\n        };\n")],
    'P7b an open refused for a home image of another size writes the failure marker': [(START,
        "            return Err(OpenError::Damaged(\n                \"the home image is not of the volume's size\",\n            ));",
        "            write_marker(dir, false);\n            return Err(OpenError::Damaged(\n                \"the home image is not of the volume's size\",\n            ));")],
    'P9 the open does not compare the size of the home image': [(START,
        "        if size != volume.size {",
        "        if size != volume.size && size == u64::MAX {")],
    'P10 a directory of the log store reads as holding no volume': [(VOL,
        "            return Err(if names.iter().any(|name| name == LEGACY_NAME) {",
        "            return Err(if names.iter().any(|name| name == LEGACY_NAME && name.is_empty()) {")],
    'P11 create does not ask for the log store\'s superblock': [
        (START,
         "    if holds(&found, LEGACY_NAME) && !holds(&found, VOLUME_NAME) {\n        return Err(VolumeError::Legacy.into());\n    }\n", ""),
        (START,
         "    if holds(&names, LEGACY_NAME) {\n        return Err(VolumeError::Legacy.into());\n    }\n", "")],
    'P12 the checksum of the volume file is not checked': [(VOL,
        "        if u32_at(56) != crc32c::crc32c(&bytes[..56]) {",
        "        if u32_at(56) != crc32c::crc32c(&bytes[..56]) && bytes.is_empty() {")],
    'P13 a longer volume file is read as this version': [(VOL,
        "        if bytes.len() != LEN || &bytes[..8] != MAGIC {",
        "        if bytes.len() < LEN || &bytes[..8] != MAGIC {")],
    'P16 the kill of the harness drops the store, as before': [(HAR,
        "        if let Inner::Live(store) = inner {\n            store.abandon();\n        }\n",
        "        drop(inner);\n")],
    'P18 the removal of an acknowledged marker is not synced': [(START,
        "            dir.remove(MARKER_NAME)\n                .and_then(|()| dir.sync())\n",
        "            dir.remove(MARKER_NAME)\n")],
    'P23 the write unit of a new volume is the page, whatever the block': [(VOL,
        "    let unit = block.max(system_write_unit());",
        "    let unit = system_write_unit().max(block.min(1));")],
    'S1 an I/O thread wakes the owner for its first result only': [
        (POOL, "    let mut after_job = false;\n", "    let mut after_job = false;\n    let mut woke = false;\n"),
        (POOL,
         "        if let Some(wake) = wake {\n            life.waking = true;",
         "        if let Some(wake) = wake.filter(|_| !std::mem::replace(&mut woke, true)) {\n            life.waking = true;")],
    'S2 a block of twice the write unit is let through': [(START,
        "        Ok(block) if block <= unit => Ok(()),",
        "        Ok(block) if block <= 2 * unit => Ok(()),")],
    'S2b the file system of the home image is not held to the unit': [(START,
        "        if blocks {\n            within_unit(Under::Home, home.block_size(), unit)?;\n        }\n", "")],
    'S3 a read that ends on a thread after the failure is answered with what it read': [(INP,
        "            } else if self.failed {\n                Err(BlkError::Failed)\n            } else if result.is_err() && self.mutants.has(Mutants::READ_ERROR_FAILS) {",
        "            } else if result.is_err() && self.mutants.has(Mutants::READ_ERROR_FAILS) {")],
    'S4 the observer is told at every failure, not once': [(INP,
        "        if let Some(observer) = self.observer.take() {\n            observer.call();\n        }\n        if !self.mutants.has(Mutants::FAIL_UNMARKED) {",
        "        if let Some(observer) = self.observer.as_ref() {\n            observer.call();\n        }\n        if !self.mutants.has(Mutants::FAIL_UNMARKED) {")],
    'S5 the lock file is made readable by all': [(FS,
        "            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);\n            let file = options.open(&path)?;\n            if let Some(lock) = finish_lock(file, &path)? {",
        "            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o644);\n            let file = options.open(&path)?;\n            if let Some(lock) = finish_lock(file, &path)? {")],
}

LOCAL = (['-p', 'skyzfs-local'],)
ALL = (['-p', 'skyzfs-local'], ['-p', 'skyzfs-ublk'], ['-p', 'skyzfsd', '--features', 'test-hooks'])
# The plants that the daemon's crates could notice too.
WIDE = {'P7', 'P7b', 'P10', 'P11', 'S1', 'S5'}


def run(RUNS=ALL):
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
    env.pop('CI', None)
    t = time.time()
    outs = []
    for args in RUNS:
        try:
            p = subprocess.run(['cargo', 'test', '-q', '--no-fail-fast', *args], cwd=W, env=env,
                               capture_output=True, text=True, timeout=2400)
        except subprocess.TimeoutExpired:
            return 'HUNG (2400 s) in ' + ' '.join(args)
        outs.append(p.stdout + p.stderr)
    out = '\n'.join(outs)
    if 'error[' in out or 'could not compile' in out:
        errs = [l for l in out.splitlines() if l.startswith('error')][:3]
        return 'DOES NOT BUILD: ' + ' | '.join(errs)
    failed = sorted(set(re.findall(r'^    (\S+)$', out, re.M)))
    passed = sum(int(n) for n in re.findall(r'test result: \w+\. (\d+) passed', out))
    return (('FAILS: ' + ', '.join(failed)) if failed else 'NOTHING FAILS') + f' ({passed} passed, {time.time() - t:.0f} s)'


def main():
    names = sys.argv[3:]
    chosen = [n for n in PLANTS if not names or any(n.split()[0] == m for m in names)]
    if not names:
        print('unplanted:', run(), flush=True)
    for name in chosen:
        saved = {}
        ok = True
        for path, old, new in PLANTS[name]:
            full = os.path.join(W, path)
            text = open(full).read()
            saved.setdefault(full, text)
            if text.count(old) != 1:
                print(f'{name}: DOES NOT APPLY ({text.count(old)} places in {path})', flush=True)
                ok = False
                break
            open(full, 'w').write(text.replace(old, new))
        try:
            if ok:
                print(f'{name}: {run(ALL if name.split()[0] in WIDE else LOCAL)}', flush=True)
        finally:
            for full, text in saved.items():
                open(full, 'w').write(text)
    st = subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout
    print('worktree after:', st.strip() or 'clean', flush=True)


main()
