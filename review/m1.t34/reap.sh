#!/bin/bash
# Does a non-interactive bash reap a background child that ended by itself?
set -u
bash -c 'exit 7' & P=$!
sleep 0.5
if kill -0 "$P" 2>/dev/null; then echo "kill -0 after exit: succeeds (not reaped)"; else echo "kill -0 after exit: fails (reaped)"; fi
if kill -9 "$P" 2>/dev/null; then echo "kill -9 after exit: succeeds"; else echo "kill -9 after exit: fails"; fi
wait "$P"; echo "wait status: $?"
# The same with a short race: the child ends just before the kill.
bash -c 'sleep 0.2; exit 7' & P=$!
sleep 0.2
kill -9 "$P" 2>/dev/null && k=ok || k=failed
wait "$P"; echo "race: kill $k, wait status $?"
