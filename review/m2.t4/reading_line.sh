#!/usr/bin/env bash
# #108 at 775814c: the line `snapshot` writes about the releaser (skzrel), run as the
# script has it, since no test runs it (plants H1 and H2 of plants2.py pass).
# A releases file as a releaser leaves it, and a background child of this shell as the
# releaser: once running, once ended while the shell waits on a foreground command.
#
#     reading_line.sh CHECKOUT
set -u
ROOT=$1
line=$(grep -n 'echo "skzrel ' "$ROOT/tests/bench/topology-bench.sh" | head -n 1 | cut -d: -f2-)
[ -n "$line" ] || { echo "no skzrel line found"; exit 2; }
OUT=$(mktemp -d)
printf '# 1794755392957548156 daemon 15558, release every 1\n1032244325043\t1\t3\n1032285098814\t3\t3\n# 1035947573132 cut 4 gone after its release was refused (refused: cut 4 is not the one being uploaded)\n' > "$OUT/skz-releases.tsv"
python3 -c 'import time; time.sleep(600)' &
SKZ_RELEASER=$!
echo "running:        $(eval "$line")"
printf '# ERROR HolderError: no reply to `held` in 30.0 s (released 2, gone 1, polls 9)\n' >> "$OUT/skz-releases.tsv"
kill "$SKZ_RELEASER"
python3 -c 'import time; time.sleep(1)'    # a foreground command, as the window is
echo "ended, unwaited: $(eval "$line")"
SKZ_RELEASER=""
echo "never started:  $(eval "$line")"
echo "(the files are left in $OUT)" >&2
