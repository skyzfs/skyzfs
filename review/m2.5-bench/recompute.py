"""recompute.py SEQDIR: the two tables' window figures again, from each row's raw .before/.after skz lines."""
import os, sys
seq = sys.argv[1]
def skz(p):
    d = {}
    for line in open(p):
        f = line.split()
        if len(f) > 2 and f[0] == 'skz':
            d.setdefault(f[1], f[2])   # first, as the row's parser keeps the last; flag duplicates below
    return d
def dupes(p):
    seen, dup = set(), set()
    for line in open(p):
        f = line.split()
        if len(f) > 2 and f[0] == 'skz':
            (dup if f[1] in seen else seen).add(f[1])
    return dup
U = ['cuts', 'uploaded', 'uploaded_whole', 'upload_blocks', 'cuts_replaced', 'cuts_abandoned', 'upload_mismatches',
     'upload_errors', 'upload_cuts_lost', 'upload_found_committed', 'upload_committed_gone', 'upload_committed_kept',
     'upload_abandoned_at_mismatch', 'upload_given_up']
I = ['intent_syncs', 'intent_sync_ns', 'intent_header_syncs', 'intent_header_sync_ns', 'intent_waits']
for row in sorted(os.listdir(seq), key=lambda x: (not x.isdigit(), int(x) if x.isdigit() else 0)):
    d = os.path.join(seq, row)
    if not os.path.isdir(d): continue
    for wl in ('tpcb', 'select'):
        b, a = os.path.join(d, f'zfs-skyzfsd-{wl}.before'), os.path.join(d, f'zfs-skyzfsd-{wl}.after')
        if not os.path.exists(a): continue
        sb, sa = skz(b), skz(a)
        if dupes(b) or dupes(a): print('  duplicate keys', dupes(b) | dupes(a))
        n = lambda k: (int(sa[k]) - int(sb[k])) if sa.get(k, '').isdigit() and sb.get(k, '').isdigit() else None
        print(f"row {row} {wl}: pid {sb.get('pid')}->{sa.get('pid')} state {sb.get('upload_state')}->{sa.get('upload_state')}"
              f" held {sb.get('cuts_held')}->{sa.get('cuts_held')} upload_cut {sb.get('upload_cut')}->{sa.get('upload_cut')} cuts {sb.get('cuts')}->{sa.get('cuts')}")
        print('   upload deltas:', {k: n(k) for k in U})
        bal = None
        if all(n(k) is not None for k in ('cuts', 'uploaded', 'cuts_replaced', 'cuts_abandoned', 'cuts_held')):
            bal = (n('uploaded') + (n('upload_found_committed') or 0) - (n('upload_committed_gone') or 0) - (n('upload_committed_kept') or 0)
                   + n('cuts_replaced') + n('cuts_abandoned') + n('cuts_held'))
            print(f"   balance: cuts {n('cuts')} vs uploaded+found-gone-kept+replaced+abandoned+dheld = {bal}")
        ms = lambda c, s: (n(s) / n(c) / 1e6) if n(c) else None
        print('   intent deltas:', {k: n(k) for k in I}, 'ms/map sync', ms('intent_syncs', 'intent_sync_ns'),
              'ms/header sync', ms('intent_header_syncs', 'intent_header_sync_ns'),
              'max', sa.get('intent_sync_max_ns'), sa.get('intent_header_sync_max_ns'),
              'named', sb.get('intent_named'), '->', sa.get('intent_named'), '/', sa.get('intent_regions'),
              'hdr', sb.get('intent_header_sync'), sa.get('intent_header_sync'), 'istate', sa.get('intent_state'))
