#!/usr/bin/env python3
"""#97 at 0f2f8c7: plants in the lines that answer N1 to N5, in the check and in the tool.

    plants3.py CHECKOUT TARGET_DIR

Each file is restored after each plant.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
CHECK = 'tests/kernel/nodeb_check.py'
TOOL = 'bins/skyzfs-nodeb/src/main.rs'
READ = ("            Err(e) if about_content(&e) => {\n"
        "                return Ok(Got::Unreadable(format!(\"read at {at}: {e}\")))\n            }\n")

PLANTS = [
    # N1 and N2: verify's every-copy branch.
    ('P1 verify: any error of the rig on a control is a catch (N1, as at 75bf72b)', CHECK,
     "            if damaged and failed == ZDB_ABORT:", "            if damaged:"),
    ('P2 verify: a failed published cut on which zdb errs is the rig\'s error (N2, as at 75bf72b)', CHECK,
     "            elif not damaged and seen.failed():", "            elif False:"),
    ('P3 verify: zdb\'s abort on the published cut is a failed stage', CHECK,
     "            if damaged and failed == ZDB_ABORT:", "            if failed == ZDB_ABORT:"),
    ('P4 verify: on a control that failed a stage, another error of the rig is not judged', CHECK,
     "            elif not damaged and seen.failed():", "            elif seen.failed():"),
    # N3: early's second condition.
    ('P5 control_verdict: the stages that must pass are not looked at', CHECK,
     "    return f'caught at {st}', st in want and not dirty, dirty", "    return f'caught at {st}', st in want, dirty"),
    ('P6 control_verdict: nothing is ever not clean', CHECK,
     "    dirty = [c for c in clean if not seen.ok(c)]", "    dirty = []"),
    ('P7 CLEAN: early need not pass its copies', CHECK,
     "CLEAN = {'early': ('import', 'copies')}", "CLEAN = {'early': ('import',)}"),
    ('P8 main: no control is given the stages that must pass', CHECK,
     "            clean = CLEAN.get(ctl, ())", "            clean = ()"),
    # N5.
    ('P9 snapshot_files: a mount that fails is the snapshot stage', CHECK,
     "                seen.put('mount', False, f'mount of", "                seen.put('snapshot', False, f'mount of"),
    ('P10 EXPECTED: lacks may be caught at the snapshot', CHECK,
     "    'lacks': ('import', 'mount', 'files', 'copies'),", "    'lacks': ('import', 'snapshot', 'mount', 'files', 'copies'),"),
    ('P11 snapshot_files: the import\'s message is cut to its end', CHECK,
     "flat(out)[:300] if rc else", "flat(out)[-300:] if rc else"),
    # N4: the tool.
    ('Q1 get: a range that cannot be read is the rig\'s error', TOOL, READ, ""),
    ('Q2 about_content: an object that is not there is not about content', TOOL,
     "            skynbd_store::StoreError::NotFound(_) | skynbd_store::StoreError::BadRange { .. }",
     "            skynbd_store::StoreError::BadRange { .. }"),
    ('Q3 about_content: every error is about content', TOOL,
     "    matches!(\n        e,\n        EngineError::Store(", "    true || matches!(\n        e,\n        EngineError::Store("),
    ('Q4 exit_code: a ref that cannot be read is 2', TOOL, "        Got::Unreadable(_) => 4,", "        Got::Unreadable(_) => 2,"),
    ('Q5 exit_code: a ref that is not there is 2', TOOL, "        Got::Absent => 3,", "        Got::Absent => 2,"),
    ('Q6 about_content: corrupt state is not about content', TOOL,
     "        ) | EngineError::Format(_)\n            | EngineError::Corrupt(_)", "        ) | EngineError::Format(_)"),
    ('Q7 about_content: an object that does not decode is not about content', TOOL,
     "        ) | EngineError::Format(_)\n            | EngineError::Corrupt(_)", "        ) | EngineError::Corrupt(_)"),
    ('Q8 about_content: a range past an object\'s end is not about content', TOOL,
     "            skynbd_store::StoreError::NotFound(_) | skynbd_store::StoreError::BadRange { .. }",
     "            skynbd_store::StoreError::NotFound(_)"),
]


def run(path):
    if path == CHECK:
        p = subprocess.run([sys.executable, '-B', os.path.join(ROOT, 'tests/kernel/test_nodeb_check.py')],
                           capture_output=True, text=True)
        failed = sorted({ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))})
        return p.returncode, failed
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-nodeb'], cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out.splitlines()
                     if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    if p.returncode and not failed:
        errs = [ln for ln in out.splitlines() if ln.startswith('error')]
        failed = ['(build) ' + (errs[0][:120] if errs else 'failed')]
    return p.returncode, failed


def main():
    for name, path, old, new in PLANTS:
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        n = orig.count(old)
        if n != 1:
            print(f'{name}: NOT APPLIED (found {n} times)', flush=True)
            continue
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        try:
            rc, failed = run(path)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
        print(f"{name}: " + ('KILLED by ' + ', '.join(failed) if rc else 'SURVIVED'), flush=True)


if __name__ == '__main__':
    main()
