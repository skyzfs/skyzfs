"""Recount the kinds from a run's raw perf samples with the pull request's perfcat.py, and look inside each kind.
    python3 -I recount.py <tests/bench dir> <perfscript.gz>"""
import collections, gzip, re, sys
sys.path.insert(0, sys.argv[1])
import perfcat as pc

text = gzip.open(sys.argv[2], 'rt', errors='replace').read()
S = pc.samples(text)
print('samples', len(S), 'no frames', sum(1 for _, f in S if not f))
threads = collections.defaultdict(list)
for comm, frames in S:
    threads[pc.name_of(comm)].append(frames)
for name, fl in sorted(threads.items(), key=lambda x: -len(x[1])):
    k = [[s for _, s in f] for f in fl if f and f[0][0] >= pc.KERNEL]
    cats = collections.Counter(pc.category(c) for c in k)
    print(f'\n== {name}: {len(fl)} samples, kernel {len(k)} ({len(k) * 100 // max(len(fl), 1)}%)')
    print('   ', ', '.join(f'{c} {n} ({n * 100 / len(k):.0f}%)' for c, n in cats.most_common()))
    # A copy routine anywhere in the chain, by the kind the sample was given.
    anyc = collections.Counter(pc.category(c) for c in k if any(pc.COPY.match(s) for s in c))
    print('    a copy routine anywhere in the chain:', dict(anyc))
    for cat in cats:
        leafs = collections.Counter(c[0] for c in k if pc.category(c) == cat)
        print(f'    [{cat}] leafs:', ', '.join(f'{s} {n}' for s, n in leafs.most_common(8)))
