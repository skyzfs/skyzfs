"""Look inside kinds: python3 -I inside.py <tests/bench> <perfscript.gz>"""
import collections, gzip, re, sys
sys.path.insert(0, sys.argv[1])
import perfcat as pc
text = gzip.open(sys.argv[2], 'rt', errors='replace').read()
T = collections.defaultdict(list)
for comm, frames in pc.samples(text):
    if frames and frames[0][0] >= pc.KERNEL:
        T[pc.name_of(comm)].append([s for _, s in frames])

def show(thread, cat, leaf=None, top=6, depth=9):
    sel = [c for c in T[thread] if pc.category(c) == cat and (leaf is None or c[0].startswith(leaf))]
    print(f'\n--- {thread} / {cat} / leaf {leaf}: {len(sel)} samples; commonest chains (leaf first, kernel part):')
    cc = collections.Counter(' < '.join(c[:depth]) for c in sel)
    for ch, n in cc.most_common(top):
        print(f'  {n:4d}  {ch}')

store = [c for c in T['skyzfs-store']]
print('store: all leafs of kind page cache:', collections.Counter(c[0] for c in store if pc.category(c) == 'page cache').most_common(60))
print('\nany symbol with copy/iter/movs/memcpy in ANY chain, by leaf-or-not:')
names = collections.Counter()
for th, cs in T.items():
    for c in cs:
        for i, s in enumerate(c):
            if re.search(r'copy|movs|memcpy|memmove|clear_page|stos', s):
                names[(s, 'leaf' if i == 0 else 'inner')] += 1
for (s, w), n in sorted(names.items(), key=lambda x: -x[1])[:40]:
    print(f'  {n:4d} {w:5s} {s}  COPY={bool(pc.COPY.match(s))}')
show('skyzfsd-queue', 'io_uring', '_raw_spin_unlock_irqrestore')
show('skyzfsd-queue', 'wake and switch', 'finish_task_switch')
show('skyzfsd-queue', 'wake and switch', '_raw_spin_unlock_irqrestore')
show('skyzfs-reader', 'wake and switch', '_raw_spin_unlock_irq')
show('skyzfs-reader', 'wake and switch', 'finish_task_switch')
show('skyzfsd-queue', 'ZFS completion', None, top=3, depth=14)
# Work under schedule() that is not the scheduler's: frames above (nearer the leaf than) the first schedule frame.
print('\nframes between the leaf and schedule()/io_schedule() in wake-and-switch samples that look like I/O or fs work:')
odd = collections.Counter()
for th, cs in T.items():
    for c in cs:
        if pc.category(c) != 'wake and switch':
            continue
        for i, s in enumerate(c):
            if re.match(r'^(schedule|io_schedule|__schedule)', s):
                above = c[:i]
                hit = [x for x in above if re.match(r'^(blk_|submit_bio|nvme|scsi|storvsc|ext4|xfs|jbd2|filemap|folio|writeback|iomap)', x)]
                if hit:
                    odd[(th, hit[-1])] += 1
                break
print(dict(odd) or 'none')
