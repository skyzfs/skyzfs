#!/usr/bin/env python3
"""plants.py WORKTREE BASE [name...]: for #57 (M1.3z). Plants a delay or a
defect in crates/skyzfs-local/src/store.rs, one at a time, and runs the
named test in its old form (the test file at BASE) and its new form (the
worktree's). Both files are put back after each plant.

A delay must fail the old test and pass the new one; a defect (a control)
must fail the new one."""
import re
import subprocess
import sys
import time

tree, base = sys.argv[1], sys.argv[2]
STORE = 'crates/skyzfs-local/src/store.rs'
TESTS = 'crates/skyzfs-local/src/store/tests.rs'
IDLE = 'every_kind_of_request_is_timed_under_its_kind'
BARRIER = 'a_steps_wait_at_a_barrier_is_counted_as_its_own_activity'

READER_TOP = '''                        let next = match read_inbox.look() {
'''
SHOW_IDLE = '''                                    show(&timing, reader.unflushed, spans.open(Activity::IDLE)),
'''
CLOSE_IDLE = '''                                    spans.close(Activity::IDLE, now, &mut timing);
                                    show(&timing, reader.unflushed, spans.open(Activity::OTHER))
'''
APPLY_TOP = '''        let began = Instant::now();
        if self.protect_neighbours(floor, target, limit).is_err() {
'''
BARRIER_CALL = '''        if !self.mutants.has(Mutants::HOME_WRITTEN_UNBARRED) {
            self.shared.barrier(self.mutants);
        }
        let saved_at = self.barred(saved_at, false);
'''
BARRED_PART = '''        barrier.add(passed - since);
        self.part(since, passed, Activity::STEP_BARRIER);
'''
SLEEP = 'std::thread::sleep(std::time::Duration::from_millis(%d));\n'

PLANTS = [
    ('d1 delay: the reader sleeps 50 ms before it looks for work', IDLE, 'delay',
     [(READER_TOP, '                        ' + SLEEP % 50 + READER_TOP)]),
    ('c1 control: d1, and the reader\'s wait is shown and closed as "other"', IDLE, 'defect',
     [(READER_TOP, '                        ' + SLEEP % 50 + READER_TOP),
      (SHOW_IDLE, SHOW_IDLE.replace('Activity::IDLE', 'Activity::OTHER')),
      (CLOSE_IDLE, CLOSE_IDLE.replace('spans.close(Activity::IDLE', 'spans.close(Activity::OTHER'))]),
    ('d2 delay: the step sleeps 20 ms before its save part', BARRIER, 'delay',
     [(APPLY_TOP, '        ' + SLEEP % 20 + APPLY_TOP)]),
    ('d3 delay: the step sleeps 150 ms before its save part', BARRIER, 'delay',
     [(APPLY_TOP, '        ' + SLEEP % 150 + APPLY_TOP)]),
    ('c2 control: the home barrier is not waited at', BARRIER, 'defect',
     [(BARRIER_CALL, BARRIER_CALL.replace('''        if !self.mutants.has(Mutants::HOME_WRITTEN_UNBARRED) {
            self.shared.barrier(self.mutants);
        }
''', ''))]),
    ('c3 control: the barrier\'s wait is counted as the save part', BARRIER, 'defect',
     [(BARRED_PART, BARRED_PART.replace('Activity::STEP_BARRIER', 'Activity::STEP_SAVE'))]),
    ('c4 control: the barrier\'s wait is not added to the barrier\'s sum', BARRIER, 'defect',
     [(BARRED_PART, BARRED_PART.replace('        barrier.add(passed - since);\n', '        let _ = barrier;\n'))]),
]


def run(test):
    began = time.time()
    out = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', test, '--', '--exact',
                          'store::tests::' + test], cwd=tree, capture_output=True, text=True)
    text = out.stdout + out.stderr
    took = time.time() - began
    m = re.search(r'^test result: (\w+)\. (\d+) passed; (\d+) failed', text, re.M)
    if not m:
        return 'DID NOT BUILD: ' + text[-400:], took
    if m.group(2) == '0' and m.group(3) == '0':
        return 'NO TEST RAN', took
    why = re.search(r'panicked at [^\n]*\n([^\n]*)', text)
    return ('ok' if m.group(1) == 'ok' else 'FAILED: ' + (why.group(1)[:150] if why else '?')), took


only = sys.argv[3:]
store, tests = f'{tree}/{STORE}', f'{tree}/{TESTS}'
orig_store, new_tests = open(store).read(), open(tests).read()
old_tests = subprocess.run(['git', 'show', f'{base}:{TESTS}'], cwd=tree, capture_output=True, text=True, check=True).stdout
for name, test, kind, edits in PLANTS:
    if only and name.split()[0] not in only:
        continue
    planted = orig_store
    for old, new in edits:
        assert planted.count(old) == 1, (name, old[:60], planted.count(old))
        planted = planted.replace(old, new)
    print(name, flush=True)
    try:
        open(store, 'w').write(planted)
        for label, body, times in (('old test', old_tests, 2 if kind == 'delay' else 1), ('new test', new_tests, 3 if kind == 'delay' else 1)):
            open(tests, 'w').write(body)
            for _ in range(times):
                said, took = run(test)
                print(f'    {label}: {said} ({took:.1f} s with the build)', flush=True)
    finally:
        open(store, 'w').write(orig_store)
        open(tests, 'w').write(new_tests)
