#!/bin/bash
# The row parser of topology-bench.sh (load, then the devices' differences), cut out of
# the script given as $1 and run on made-up snapshots of eight and of seven values.
set -eu
S=$1
d=$(mktemp -d)
{
  echo "import sys"
  echo "MB = float(1 << 20)"
  echo "zc, zg = {}, {}; skz = {}"
  echo "cpu, thr, txgs, mem, loops, ext = {}, {}, {}, {}, {}, {}"
  sed -n '/^def load(p):/,/^    return dev, pg, other/p' "$S"
  echo "bd, bp, bo = load(sys.argv[1]); ad, ap, ao = load(sys.argv[2])"
  echo "r = {'devices': {}}"
  sed -n '/^for name in ad:/,/^for k in ap:/p' "$S" | sed '$d'
  echo "v = r['devices']['workdisk']"
  echo "print({k: v[k] for k in ('read_ios', 'write_ios', 'flushes', 'write_ms', 'flush_ms', 'read_ms') if k in v})"
} > "$d/cut.py"
printf 'dev workdisk 1000 8000 5000 40000 100 9000 300 2000\n' > "$d/b8"
printf 'dev workdisk 1400 16000 7000 80000 150 15000 450 2600\n' > "$d/a8"
printf 'dev workdisk 1000 8000 5000 40000 100 9000 300\n' > "$d/b7"
printf 'dev workdisk 1400 16000 7000 80000 150 15000 450\n' > "$d/a7"
echo "eight values: $(python3 -I "$d/cut.py" "$d/b8" "$d/a8")"
echo "seven values: $(python3 -I "$d/cut.py" "$d/b7" "$d/a7")"
rm -rf "${d:?}"
