import importlib.util, sys, collections
# argv: judge script, rig-out dir, output tsv
spec = importlib.util.spec_from_file_location('dc', sys.argv[1])
dc = importlib.util.module_from_spec(spec); spec.loader.exec_module(dc)
checks, _ = dc.parse_checks(dc.DEFAULT_CHECKS)
results, twice = dc.read_results(sys.argv[2] + '/results.tsv')
impl = 'reference' if results.get('impl', '').startswith('reference') else 'daemon'
store, se = "n/a (this judge has no store_of)", []
rows, differs, errors = dc.judge(results, checks, impl)
out = ['name\tverdict\tas-expected or differs']
cnt = collections.Counter()
for name, want, got, detail, basis, claim in rows:
    st = 'as-expected' if got == want else ('not run (reference only)' if got == 'not run' else f'differs (expected {want})')
    out.append(f'{name}\t{got}\t{st}')
    cnt[got] += 1
nobs = 0
for name, (check, q) in dc.OBSERVED.items():
    if check in checks and name in results:
        out.append(f'{name}\tobservation\tnot judged'); nobs += 1
open(sys.argv[3], 'w').write('\n'.join(out) + '\n')
pc = sum(1 for r in rows if r[0].startswith('pin_')); cc = sum(1 for r in rows if r[0].startswith('ctl_')); kc = sum(1 for r in rows if r[0].startswith('case_'))
print(sys.argv[3].rsplit('/',1)[1], dict(cnt), f'rows {len(rows)} (pin {pc}, ctl {cc}, case {kc}); observations {nobs}; differs {len(differs)}; errors {len(errors)+len(twice)+len(se)}; store {store}')
for d in differs: print('  DIFFERS', d)
for e in errors + twice + se: print('  ERROR', e)
