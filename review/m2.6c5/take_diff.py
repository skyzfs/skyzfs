import sys, os, collections
for art in sys.argv[1:]:
    for pool in ('rig', 'mirror'):
        a = open(os.path.join(art, f'{pool}-odd-get-local', 'stdout'), 'rb').read().split(b'\n')
        b = open(os.path.join(art, f'{pool}-odd-get-received', 'stdout'), 'rb').read().split(b'\n')
        bb = []
        for l in b:
            if l.endswith(b'\treceived'):
                bb.append(b'RECEIVED:' + l); continue
            bb.append(l[:-len(b'\tlocal')] if l.endswith(b'\tlocal') else l)
        ca, cb = collections.Counter(a), collections.Counter(bb)
        only_a = [l for l in (ca - cb).elements()]
        only_b = [l for l in (cb - ca).elements()]
        props = collections.Counter()
        for l in only_a + only_b:
            f = l.split(b'\t')
            props[(b'B ' if l in only_b else b'A ') + (f[1] if len(f) > 1 else b'?')] += 1
        print(os.path.basename(os.path.dirname(art)), pool, dict(props))
        for l in only_a + only_b:
            f = l.split(b'\t')
            if len(f) > 1 and f[1] not in (b'snapshot_count', b'filesystem_count'):
                print('   ', 'A' if l in only_a else 'B', l[:120])
