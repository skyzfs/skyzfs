#!/bin/bash
# Build the test binaries the timing runs use, at the head and with
# plants, and keep copies of them; the worktree is restored after each.
# Usage: bins.sh WORKTREE OUTDIR TARGETDIR
W=$1; O=$2; T=$3
cd "$W" || exit 2
export CARGO_TARGET_DIR=$T CARGO_INCREMENTAL=0
mkdir -p "$O/bin"
CTT=bins/skyzfsd/src/control/tests.rs
CT=bins/skyzfsd/src/control.rs
FS=crates/skyzfs-local/src/fs.rs
exe_of() { # cargo args...: print the lib test executable
  cargo test --no-run --message-format=json "$@" 2>>"$O/bins.log" \
    | python3 -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("profile",{}).get("test") and m.get("executable") and "lib" in m["target"]["kind"]: print(m["executable"])' | tail -1
}
keep() { local name=$1; shift; local e; e=$(exe_of "$@"); if [ -n "$e" ]; then cp "$e" "$O/bin/$name"; echo "$name <- $(basename "$e")" >> "$O/bins.log"; else echo "$name: NO BINARY" >> "$O/bins.log"; fi; }
plant() { python3 - "$W/$1" "$2" "$3" <<'PY'
import sys
p, old, new = sys.argv[1:4]
t = open(p).read(); assert t.count(old) == 1, old
open(p, "w").write(t.replace(old, new))
PY
}
: > "$O/bins.log"
keep l_head -p skyzfs-local --lib
keep d_head -p skyzfsd --lib
# The review's room scenario, inserted before the test that follows the new one.
anchor='    /// No daemon at the socket (a socket left by one that has gone, or a'
python3 - "$W/$CTT" "$O/scenario_room.rs" "$anchor" <<'PY'
import sys
p, s, a = sys.argv[1:4]
t = open(p).read(); assert t.count(a) == 1
open(p, "w").write(t.replace(a, open(s).read() + a))
PY
keep room_head -p skyzfsd --lib
CAP='                pause = (pause * 2).min(CONNECT_PAUSE);'
plant $CT "$CAP" '                pause = (pause * 2).min(CONNECT_PAUSE * 1000);'
keep room_CE -p skyzfsd --lib
git checkout -- $CT
plant $CT "$CAP" '                pause = (pause * 2).min(CONNECT_PAUSE * 10);'
keep room_CE5 -p skyzfsd --lib
git checkout -- $CT
plant $CT "$CAP" '                pause = (pause * 2).min(CONNECT_PAUSE * 8);'
keep room_CE8 -p skyzfsd --lib
git checkout -- $CT
plant $CT '    let mut pause = Duration::from_millis(1);' '    let mut pause = CONNECT_PAUSE;'
keep room_CF -p skyzfsd --lib
git checkout -- $CT
git checkout -- $CTT
# The join of the file's helper removed (J).
plant $FS '            let _ = thread.join();' '            drop(thread);'
keep l_J -p skyzfs-local --lib
git checkout -- $FS
git status --short >> "$O/bins.log"
echo done >> "$O/bins.log"
