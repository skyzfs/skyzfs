import collections, gzip, re, sys
sys.path.insert(0, sys.argv[2])
import perfcat as pc
text = gzip.open(sys.argv[1], 'rt', errors='replace').read()
kern = [(pc.name_of(c), [s for _, s in fr]) for c, fr in pc.samples(text) if fr and fr[0][0] >= pc.KERNEL]
STRICT = re.compile(r'^(try_to_free_mem_cgroup_pages|try_to_free_pages|do_try_to_free_pages|shrink_node|shrink_lruvec|shrink_folio_list|'
                    r'shrink_one|shrink_many|evict_folios|mem_cgroup_handle_over_high|reclaim_high|shrink_slab|try_to_shrink_lruvec)')
tot = collections.Counter(t for t, _ in kern)
rec = collections.Counter(); weak = collections.Counter(); kinds = collections.Counter()
for t, ch in kern:
    if pc.category(ch) == 'page reclaim':
        rec[t] += 1
        if not any(STRICT.match(s) for s in ch):
            weak[t] += 1
            kinds['refault' if any(s in ('workingset_refault', 'lru_gen_refault') or s == 'lru_gen_test_recent' for s in ch) and 'lru_add' not in ch else ('lru_add' if 'lru_add' in ch else 'else')] += 1
for t in rec:
    print(f"{t}: page reclaim {rec[t]} of {tot[t]} ({100*rec[t]/tot[t]:.1f}%), without an entry point {weak[t]}, left {rec[t]-weak[t]} ({100*(rec[t]-weak[t])/tot[t]:.1f}%)")
print(dict(kinds))
# the same with lru_gen_ taken out of the rule
pc.RECLAIM = re.compile(pc.RECLAIM.pattern.replace('lru_gen_|', ''))
after = collections.Counter((t, pc.category(ch)) for t, ch in kern)
print('with lru_gen_ out: queue', {c: n for (t, c), n in after.items() if t == 'skyzfsd-queue' and c in ('page reclaim', 'memory group', 'page cache')})
print('\nwrite-back from a write, leaf and chain:')
pc.RECLAIM = re.compile(pc.RECLAIM.pattern)
for t, ch in kern:
    if pc.category(ch) == 'write-back from a write':
        print('  ', t, ' < '.join(ch[:6]))
print('samples with balance_dirty_pages itself:', sum('balance_dirty_pages' in ch for _, ch in kern),
      ' chains with a write-back symbol:', sum(any(pc.WRITEBACK.match(s) for s in ch) for _, ch in kern),
      ' of those under a write call:', sum(any(pc.WRITEBACK.match(s) for s in ch) and any(pc.WRITE_CALL.match(s) for s in ch) for _, ch in kern))
print('count_memcg_events not under a memory group frame:', sum('count_memcg_events' in ch and pc.category(ch) not in ('memory group', 'page reclaim') for _, ch in kern))
