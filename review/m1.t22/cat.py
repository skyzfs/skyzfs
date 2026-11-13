import collections, gzip, re, sys
sys.path.insert(0, sys.argv[2])   # the repository's tests/bench, not the data directory
import perfcat as pc
text = gzip.open(sys.argv[1], 'rt', errors='replace').read()
ss = pc.samples(text)
kern = [(pc.name_of(c), [s for _, s in fr]) for c, fr in ss if fr and fr[0][0] >= pc.KERNEL]
print('samples', len(ss), 'kernel', len(kern))
cats = collections.Counter((t, pc.category(ch)) for t, ch in kern)
tot = collections.Counter(t for t, _ in kern)
for t in sorted(tot, key=lambda t: -tot[t]):
    print(f"{t}: {tot[t]} kernel samples: " + ', '.join(f"{c} {n} ({100*n/tot[t]:.1f}%)" for (tt, c), n in cats.most_common() if tt == t))
STRICT = re.compile(r'^(try_to_free_mem_cgroup_pages|try_to_free_pages|do_try_to_free_pages|shrink_node|shrink_lruvec|shrink_folio_list|'
                    r'shrink_one|shrink_many|evict_folios|mem_cgroup_handle_over_high|reclaim_high|shrink_slab|try_to_shrink_lruvec|balance_pgdat|kswapd)')
def without(name):
    for n, rule in pc.CATEGORIES:
        if n != name and rule(ch):
            return n
print('\n== page reclaim: by the reclaim symbols in the chain that are not from a reclaim entry point')
weak = collections.Counter(); strict = 0; other_if = collections.Counter()
for t, ch in kern:
    if pc.category(ch) != 'page reclaim': continue
    if any(STRICT.match(s) for s in ch): strict += 1
    else:
        weak[tuple(s for s in ch if pc.RECLAIM.match(s))] += 1
        other_if[without('page reclaim')] += 1
print('with a reclaim entry point in the chain:', strict, ' without:', sum(weak.values()), dict(other_if))
for k, n in weak.most_common(12): print('  ', n, k)
print('\n== page reclaim: what is above the outermost reclaim frame (who asked)')
above = collections.Counter()
for t, ch in kern:
    if pc.category(ch) != 'page reclaim': continue
    idx = max(i for i, s in enumerate(ch) if pc.RECLAIM.match(s))
    above[(t, ' < '.join(ch[idx:idx + 4]))] += 1
for k, n in above.most_common(8): print('  ', n, k)
print('\n== memory group: what the sample would be without the kind, and the frame above the outermost memcg frame')
alt = collections.Counter(); ctx = collections.Counter(); leaf = collections.Counter()
for t, ch in kern:
    if pc.category(ch) != 'memory group': continue
    alt[without('memory group')] += 1
    idx = max(i for i, s in enumerate(ch) if pc.MEMCG.match(s))
    ctx[' < '.join(ch[idx:idx + 3])] += 1
    leaf[ch[0]] += 1
    if any(pc.DIRTY_THROTTLE.match(s) or pc.WRITEBACK.match(s) for s in ch): alt['(has throttle or write-back above)'] += 1
print(dict(alt))
for k, n in ctx.most_common(12): print('  ', n, k)
print('  leaves:', leaf.most_common(10))
print('\n== write-back from a write')
wb = collections.Counter()
for t, ch in kern:
    if pc.category(ch) != 'write-back from a write': continue
    thr = [s for s in ch if pc.DIRTY_THROTTLE.match(s)]
    wb[('throttle: ' + ','.join(thr)) if thr else 'write-back under a write: ' + ' < '.join(s for s in ch if pc.WRITEBACK.match(s) or pc.WRITE_CALL.match(s))] += 1
for k, n in wb.most_common(10): print('  ', n, k)
print('\n== in other kinds: symbols that look like reclaim, memory group or LRU and that no new rule matched')
HINT = re.compile(r'memcg|cgroup|lru|reclaim|shrink|workingset|page_counter|charge|rstat|dirty|writeback|swap')
miss = collections.Counter()
for t, ch in kern:
    c = pc.category(ch)
    if c in ('page reclaim', 'memory group', 'write-back from a write'): continue
    for s in set(ch):
        if HINT.search(s): miss[(c, s)] += 1
for k, n in miss.most_common(25): print('  ', n, k)
print('\n== copies whose chain has reclaim, memory group or throttle (stay copies)')
cp = collections.Counter()
for t, ch in kern:
    c = pc.category(ch)
    if c.startswith('copy') and any(pc.RECLAIM.match(s) or pc.MEMCG.match(s) or pc.DIRTY_THROTTLE.match(s) for s in ch):
        cp[(c, tuple(s for s in ch if pc.RECLAIM.match(s) or pc.MEMCG.match(s) or pc.DIRTY_THROTTLE.match(s))[:3])] += 1
for k, n in cp.most_common(8): print('  ', n, k)
