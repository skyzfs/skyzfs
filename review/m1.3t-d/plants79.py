"""Plants for #79: each applied alone to bins/skyzfsd/src/cli.rs at ba29846, then the cli tests run."""
import subprocess, sys
W = '/home/user/skyzfs-pr79'
F = W + '/bins/skyzfsd/src/cli.rs'
ENV = {'CARGO_TARGET_DIR': W + '/target'}
OPT = '    opt("--io-threads", true, false),\n'
PLANTS = [
    ('T1 serve takes --store again, any value, and ignores it',
     [(OPT, '    opt("--store", true, false),\n' + OPT)]),
    ('T2 the usage offers [--store in-place] again',
     [('                 [--io-threads N] [--kernel-has-async-delete]\n                 (one queue',
       '                 [--io-threads N] [--kernel-has-async-delete] [--store in-place]\n                 (one queue')]),
    ('T3 the help offers [--store in-place] again',
     [('                [--io-threads N] [--kernel-has-async-delete]\n      Serve the volume',
       '                [--io-threads N] [--kernel-has-async-delete] [--store in-place]\n      Serve the volume')]),
    ('T4 the hooks usage names --store',
     [('[--commit-delay-ms N] [--request-log PATH]";', '[--commit-delay-ms N] [--request-log PATH] [--store]";')]),
    ('T5 a comment in cli.rs says [--store',
     [('//! There is no choice of a store:', '//! (Once: [--store in-place].) There is no choice of a store:')]),
]
orig = open(F).read()
for name, edits in PLANTS:
    s = orig
    for old, new in edits:
        assert s.count(old) == 1, (name, old)
        s = s.replace(old, new)
    open(F, 'w').write(s)
    try:
        r = subprocess.run(['cargo', 'test', '-q', '-p', 'skyzfsd', '--features', 'skyzfsd/test-hooks', '--lib', 'cli::'],
                           cwd=W, env={**__import__('os').environ, **ENV}, capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = [l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')]
        failed += [l.strip() for l in out.splitlines() if l.startswith('    cli::')]
        res = [l for l in out.splitlines() if l.startswith('test result')]
        errs = [l for l in out.splitlines() if l.startswith('error')]
        print(f'{name}: rc {r.returncode}; failed {sorted(set(failed))}; {res}; build errors {errs[:3]}', flush=True)
    finally:
        open(F, 'w').write(orig)
subprocess.run(['git', 'diff', '--quiet'], cwd=W, check=True)
print('clean')
