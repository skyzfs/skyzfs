import os, sys, collections
for art in sys.argv[1:]:
    props = collections.Counter(); where = collections.defaultdict(set)
    for name in sorted(os.listdir(art)):
        p = os.path.join(art, name)
        try:
            cmd = open(os.path.join(p, 'cmd'), 'rb').read().decode('utf-8', 'replace')
        except Exception:
            continue
        if 'zfs get' not in cmd or '-s local' not in cmd and 'local' not in cmd:
            continue
        if '-o value ' in cmd: continue
        out = open(os.path.join(p, 'stdout'), 'rb').read()
        for line in out.split(b'\n'):
            f = line.split(b'\t')
            if len(f) >= 2:
                prop = f[1].decode('utf-8', 'replace')
                if len(f) >= 4 and f[3] not in (b'local',):
                    # columns with source: keep only local
                    if b'local' not in f[3:]:
                        continue
                props[prop] += 1; where[prop].add(name)
    print('==', art)
    for k, v in sorted(props.items()):
        print(f'{k:28} {v:5}  e.g. {sorted(where[k])[:4]}')
