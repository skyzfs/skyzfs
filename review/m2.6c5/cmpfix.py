import json, sys, os
fix, art = sys.argv[1], sys.argv[2]
d = json.load(open(fix))
bad = 0
for name, rec in sorted(d['records'].items()):
    p = os.path.join(art, name)
    if not os.path.isdir(p):
        print('MISSING in artifact', name); bad += 1; continue
    rd = lambda f: open(os.path.join(p, f), 'rb').read() if os.path.exists(os.path.join(p, f)) else None
    out = rd('stdout'); err = rd('stderr'); rc = rd('rc'); cmd = rd('cmd')
    want_out = bytes.fromhex(rec['stdout_hex']) if 'stdout_hex' in rec else rec['stdout'].encode('utf-8', 'surrogateescape')
    diffs = []
    if out != want_out: diffs.append(f'stdout {len(out or b"")} vs {len(want_out)}')
    if 'stdout_hex' in rec and 'stdout' in rec and rec['stdout'].encode('utf-8','surrogateescape') != out:
        diffs.append('stdout text differs from bytes (lossy ok?)')
    if (err or b'').decode('utf-8','replace') != rec.get('stderr', ''): diffs.append(f'stderr {err!r} vs {rec.get("stderr")!r}')
    if rc is not None and rc.decode().strip() != str(rec['rc']).strip(): diffs.append(f'rc {rc!r} vs {rec["rc"]!r}')
    if cmd is not None and cmd.decode() != rec['cmd'] and cmd.decode().strip() != rec['cmd'].strip(): diffs.append(f'cmd {cmd!r} vs {rec["cmd"]!r}')
    print(name, 'OK' if not diffs else 'DIFF ' + '; '.join(diffs))
    bad += bool(diffs)
print('records', len(d['records']), 'bad', bad)
