#!/usr/bin/env bash
# Review probe (Q10, third look): what the stall line says when the
# writer was not writing during the window. W = the head's dsync_writer.py.
W=$1 D=$2
mkdir -p "$D"
echo "== X1: the writer killed (SIGKILL) 1 s after it started; the window is after its death"
python3 "$W" write "$D/x1.dat" "$D/x1.tsv" 2> "$D/x1.err" &
pid=$!
sleep 1
kill -KILL "$pid"; wait "$pid" 2> /dev/null
t0=$(date +%s.%N); sleep 2; t1=$(date +%s.%N)
python3 "$W" stall "$D/x1.tsv" "$t0" "$t1"; echo "   stall exit $?"
echo "   log kinds: $(cut -f1 "$D/x1.tsv" | sort | uniq -c | tr -s ' ' | tr '\n' ' ')"
echo "== X2: the writer's writes fail at once (ENOSPC, /dev/full); the window is while it 'ran'"
t0=$(date +%s.%N)
python3 "$W" write /dev/full "$D/x2.tsv" 2> "$D/x2.err"; echo "   writer exit $?: $(tail -n 1 "$D/x2.err")"
sleep 1; t1=$(date +%s.%N)
python3 "$W" stall "$D/x2.tsv" "$t0" "$t1"; echo "   stall exit $?"
echo "== X3: a healthy writer across the same kind of window, for comparison"
python3 "$W" write "$D/x3.dat" "$D/x3.tsv" 2> "$D/x3.err" &
pid=$!
sleep 0.5; t0=$(date +%s.%N); sleep 2; t1=$(date +%s.%N); sleep 0.5
kill -TERM "$pid"; wait "$pid"
python3 "$W" stall "$D/x3.tsv" "$t0" "$t1"; echo "   stall exit $?"
echo "== X4: the writer started 1.5 s after the window began (late start)"
t0=$(date +%s.%N)
sleep 1.5
python3 "$W" write "$D/x4.dat" "$D/x4.tsv" 2> "$D/x4.err" &
pid=$!
sleep 1; t1=$(date +%s.%N); sleep 0.3
kill -TERM "$pid"; wait "$pid"
python3 "$W" stall "$D/x4.tsv" "$t0" "$t1"; echo "   stall exit $?"
echo "== X5: no log at all (the writer could not open its file)"
python3 "$W" write "$D/nodir/x5.dat" "$D/x5.tsv" 2> "$D/x5.err"; echo "   writer exit $?"
python3 "$W" stall "$D/x5.tsv" 1 2 2>&1 | tail -n 1; echo "   stall exit ${PIPESTATUS[0]}"
