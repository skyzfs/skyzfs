import gzip, sys
sys.path.insert(0, sys.argv[2])
import perfcat as pc
n = 0
for p in sys.argv[3:]:
    text = gzip.open(p, 'rt', errors='replace').read()
    for c, fr in pc.samples(text):
        if fr and fr[0][0] >= pc.KERNEL:
            ch = [s for _, s in fr]
            if any(s.startswith(('try_to_free', 'shrink_node', 'do_try_to_free')) for s in ch) and any(pc.MEMCG.match(s) for s in ch):
                n += 1
print('chains with a reclaim entry point and a memory group frame:', n)
