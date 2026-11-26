#!/bin/bash
# Drive the real binary's `run` (debug build at bb03454) with fake zfs and
# zpool on PATH that fail after a sleep, and signal it. Cases a-e have no
# daemon (as review/m2.7a/next_look/live.sh); cases f and g have a fake
# daemon that answers `cuts` with a newest cut that does not open as
# itself (opens=rewind), so that each round goes through Agent::sync_past.
# Each case logs its calls with their times relative to the first signal.
S=$SCRATCH
B=$S/r170/bin/skyzfs-agent
T=$S/r170/live; rm -rf $T; mkdir -p $T/bin
for p in zfs zpool; do
cat > $T/bin/$p <<X
#!/bin/sh
echo "\$(date +%s.%N) start $p \$*" >> \$CALLS
sleep \${FAKE_SLEEP:-0}
echo "\$(date +%s.%N) end $p \$*" >> \$CALLS
echo "fake $p: refused" >&2
exit 1
X
chmod +x $T/bin/$p; done
cat > $T/daemon.py <<'X'
# A fake skyzfsd control socket: answers every `cuts` with a newest cut of
# group 40 that does not open as itself, the count one higher each time.
import os, socket, sys, time
path, calls = sys.argv[1], sys.argv[2]
os.makedirs(os.path.dirname(path), exist_ok=True)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.bind(path); s.listen(8)
n = 5
while True:
    c, _ = s.accept()
    data = b''
    while data.count(b'\n') < 2:
        b = c.recv(4096)
        if not b: break
        data += b
    line = data.split(b'\n')[1].decode() if data.count(b'\n') >= 2 else '?'
    with open(calls, 'a') as f: f.write(f'{time.time():.9f} start daemon {line}\n')
    if line == 'cuts':
        n += 1
        ans = f'cuts attachment={"ab"*16} state={"cd"*16} count={n} group=40 opens=rewind'
    else:
        ans = 'refused malformed fake'
    c.sendall(f'skyzfs-control 2\n{ans}\n'.encode()); c.close()
X
case_() { # name sleep every daemon signals...
  name=$1; sl=$2; ev=$3; dm=$4; shift 4
  d=$T/$name; mkdir -p $d/vol $d/dir
  # The run directory is kept short: a socket's path is at most 107 bytes.
  run=$S/r/${name%%_*}; rm -rf $run; mkdir -p $run
  printf 'skyzfs-remote 1\nendpoint http://127.0.0.1:9000\nbucket volumes\nvolume pg-data\nid 01ARZ3NDEKTSV4RRFFQ69G5FAV\n' > $d/vol/remote
  : > $d/calls
  dpid=
  if [ "$dm" = yes ]; then
    sock=$(printf '%s/v-%x-%x/control' "$run" "$(stat -c %d "$d/vol")" "$(stat -c %i "$d/vol")")
    python3 -I $T/daemon.py "$sock" $d/calls & dpid=$!
    for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.1; done
  fi
  t0=$(date +%s.%N)
  CALLS=$d/calls PATH=$T/bin:$PATH FAKE_SLEEP=$sl $B run --volume $d/vol --run $run --device /dev/null --pool tank --dir $d/dir --every-s $ev > $d/out 2> $d/err &
  pid=$!
  for step in "$@"; do
    sig=${step%%@*}; at=${step##*@}
    sleep $at; kill -$sig $pid 2>/dev/null; echo "$(date +%s.%N) sent $sig" >> $d/sig
  done
  if [ $# -eq 0 ]; then sleep 4; kill -TERM $pid; echo "$(date +%s.%N) sent TERM" >> $d/sig; fi
  wait $pid; rc=$?
  t1=$(date +%s.%N)
  [ -n "$dpid" ] && kill $dpid 2>/dev/null
  first=$(head -1 $d/sig | cut -d' ' -f1)
  echo "== $name: rc=$rc elapsed=$(echo "$t1 - $t0" | bc)"
  echo "   last signal to exit: $(echo "$t1 - $(tail -1 $d/sig | cut -d' ' -f1)" | bc) s"
  sed 's/^/   out: /' $d/out | tail -4
  sed 's/^/   err: /' $d/err | tail -6
  echo "   calls started: $(grep -c ' start ' $d/calls); started after the first signal: $(awk -v t=$first '$2=="start" && $1>t' $d/calls | wc -l)"
  sort -n $d/calls | awk -v t=$first '{ rel=$1-t; $1=""; printf "   call %+.2f s%s\n", rel, $0 }'
}
case_ a_term_waiting 0 5 no TERM@3
case_ b_int_waiting 0 5 no INT@3
case_ c_term_in_call 3 60 no TERM@1
case_ d_twice_in_call 3 60 no TERM@1 TERM@0.5
case_ e_term_int_in_call 3 60 no TERM@1 INT@0.5
case_ f_behind_no_signal_until_4s 0 60 yes
case_ g_behind_term_in_premise 3 60 yes TERM@1
