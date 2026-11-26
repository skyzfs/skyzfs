#!/bin/bash
# Drive the real binary's `run` with fake zfs/zpool on PATH (no daemon), and signal it.
S=$SCRATCH
B=$S/t167/debug/skyzfs-agent
T=$S/r167/live; rm -rf $T; mkdir -p $T/bin
for p in zfs zpool; do
cat > $T/bin/$p <<X
#!/bin/sh
echo "\$(date +%s.%N) $p \$*" >> $T/calls.log
sleep \${FAKE_SLEEP:-0}
echo "fake $p: refused" >&2
exit 1
X
chmod +x $T/bin/$p; done
case_() { # name sleep every signals-delay-list
  name=$1; sl=$2; ev=$3; shift 3
  d=$T/$name; mkdir -p $d/vol $d/run $d/dir
  printf 'skyzfs-remote 1\nendpoint http://127.0.0.1:9000\nbucket volumes\nvolume pg-data\nid 01ARZ3NDEKTSV4RRFFQ69G5FAV\n' > $d/vol/remote
  : > $T/calls.log
  t0=$(date +%s.%N)
  PATH=$T/bin:$PATH FAKE_SLEEP=$sl $B run --volume $d/vol --run $d/run --device /dev/null --pool tank --dir $d/dir --every-s $ev > $d/out 2> $d/err &
  pid=$!
  for step in "$@"; do
    sig=${step%%@*}; at=${step##*@}
    sleep $at; kill -$sig $pid 2>/dev/null; echo "$(date +%s.%N) sent $sig" >> $d/sig
  done
  wait $pid; rc=$?
  t1=$(date +%s.%N)
  echo "== $name: rc=$rc elapsed=$(echo "$t1 - $t0" | bc)"
  cat $d/sig 2>/dev/null | sed 's/^/   /'
  echo "   last signal to exit: $(echo "$t1 - $(tail -1 $d/sig | cut -d' ' -f1)" | bc) s"
  sed 's/^/   out: /' $d/out | tail -4
  sed 's/^/   err: /' $d/err | tail -4
  echo "   zfs calls: $(wc -l < $T/calls.log); after the first signal: $(awk -v t=$(head -1 $d/sig | cut -d' ' -f1) '$1>t' $T/calls.log | wc -l)"
}
case_ term_waiting 0 5 TERM@3
case_ int_waiting 0 5 INT@3
case_ term_in_call 3 60 TERM@1
case_ twice_in_call 3 60 TERM@1 TERM@0.5
case_ term_int_in_call 3 60 TERM@1 INT@0.5
