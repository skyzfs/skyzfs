#!/usr/bin/env python3
"""plants.py WORKTREE: plant defects in #55's code, one at a time, run the
crate's tests and say which tests fail. Each file is put back after its
plant. A plant names the occurrence of OLD to change (0 = the first) and
how many there must be."""
import re
import subprocess
import sys

tree = sys.argv[1]
SERVE = 'bins/skyzfsd/src/serve.rs'
INPLACE = 'crates/skyzfs-local/src/inplace.rs'
LIFE = 'crates/skyzfs-ublk/src/lifecycle.rs'
REC = 'crates/skyzfs-ublk/src/records.rs'
PLANTS = [
    ('s1 the failed start notes the log\'s reason whatever failed', SERVE,
     '                    start_failure(start.failed()),\n', '                    Reason::LogFailed,\n', 0, 1,
     ['-p', 'skyzfsd']),
    ('s2 a failure while serving (the loop) notes the log\'s reason', SERVE,
     '            served_failure(serve.store),\n', '            Reason::LogFailed,\n', 0, 2,
     ['-p', 'skyzfsd']),
    ('s3 a failure with the queue\'s end notes the log\'s reason', SERVE,
     '            served_failure(serve.store),\n', '            Reason::LogFailed,\n', 1, 2,
     ['-p', 'skyzfsd']),
    ('s4 the in-place store serving gives the log\'s reason', SERVE,
     '        StoreKind::InPlace => Reason::ImageFailed,\n', '        StoreKind::InPlace => Reason::LogFailed,\n', 0, 1,
     ['-p', 'skyzfsd']),
    ('s5 a start the log store failed gives the image\'s reason', SERVE,
     '        StartFailed::Log => Reason::LogFailed,\n', '        StartFailed::Log => Reason::ImageFailed,\n', 0, 1,
     ['-p', 'skyzfsd']),
    ('i1 the in-place start\'s own sync says Log', INPLACE,
     '                failed: StartFailed::Image,\n', '                failed: StartFailed::Log,\n', 0, 1,
     ['-p', 'skyzfs-local', '--lib', 'inplace']),
    ('i2 a start the log store failed says Image', INPLACE,
     '            failed: StartFailed::Log,\n', '            failed: StartFailed::Image,\n', 0, 1,
     ['-p', 'skyzfs-local', '--lib', 'inplace']),
    ('l1 note_failure writes the log\'s reason', LIFE,
     '        reason,\n        identity,\n    };\n    let written = records.write_failure(&failure).is_ok();',
     '        reason: Reason::LogFailed,\n        identity,\n    };\n    let _ = reason;\n    let written = records.write_failure(&failure).is_ok();', 0, 1,
     ['-p', 'skyzfs-ublk']),
    ('l2 the image\'s words are the log\'s', LIFE,
     'Reason::ImageFailed => "a write or sync of the volume\'s image failed",',
     'Reason::ImageFailed => "the log failed while the volume was served",', 0, 1,
     ['-p', 'skyzfs-ublk']),
    ('r1 code 7 is written as 9', REC,
     '    (7, Reason::ImageFailed),\n', '    (9, Reason::ImageFailed),\n', 0, 1,
     ['-p', 'skyzfs-ublk']),
    ('r2 an unknown code reads as the log\'s', REC,
     '    let (_, reason) = REASONS.iter().find(|(code, _)| *code == bytes[41])?;',
     '    let (_, reason) = REASONS.iter().find(|(code, _)| *code == bytes[41]).unwrap_or(&REASONS[0]);', 0, 1,
     ['-p', 'skyzfs-ublk']),
]
only = sys.argv[2:]
for name, rel, old, new, nth, count, test in PLANTS:
    if only and name.split()[0] not in only:
        continue
    path = f'{tree}/{rel}'
    orig = open(path).read()
    assert orig.count(old) == count, (name, orig.count(old))
    parts = orig.split(old)
    changed = old.join(parts[:nth + 1]) + new + old.join(parts[nth + 1:])
    open(path, 'w').write(changed)
    try:
        run = subprocess.run(['cargo', 'test', *test], cwd=tree, capture_output=True, text=True, timeout=1500)
        out = run.stdout + run.stderr
        fails = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)))
        errors = re.findall(r'^error(?:\[E\d+\])?: .*', out, re.M)[:2]
        passed = sum(int(n) for n in re.findall(r'^test result: \w+\. (\d+) passed', out, re.M))
        if fails:
            print(f'{name}: FAILS {"; ".join(fails)}')
        elif errors:
            print(f'{name}: DOES NOT BUILD: {errors}')
        else:
            print(f'{name}: NOTHING FAILS ({passed} passed, exit {run.returncode})')
    except subprocess.TimeoutExpired:
        print(f'{name}: the tests did not end')
    finally:
        open(path, 'w').write(orig)
    sys.stdout.flush()
print(subprocess.run(['git', 'status', '--short'], cwd=tree, capture_output=True, text=True).stdout or 'clean')
