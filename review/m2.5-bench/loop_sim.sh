#!/usr/bin/env bash
# loop_sim.sh ROOT SEQUENCE: the workflow's "Run benchmark" step (from ROOT's
# topology-bench.yml) run under `bash -e` as the runner runs it, with sudo,
# docker, the bench script and minio.sh made up. The made-up bench script
# exits 64 BEFORE making TB_OUT (as a refused row does) when its assigns name
# TB_REFUSE=1, and otherwise makes TB_OUT and exits 0.
set -uo pipefail
ROOT=$1; SEQ=$2
T=$(mktemp -d); trap 'rm -rf -- "$T"' EXIT
mkdir -p "$T/repo/tests/bench" "$T/bin" "$T/out" "$T/work" "$T/minio"
python3 -I - "$ROOT/.github/workflows/topology-bench.yml" > "$T/step.sh" <<'PY'
import sys, yaml
wf = yaml.safe_load(open(sys.argv[1]))
for s in wf['jobs']['bench']['steps']:
    if s.get('name') == 'Run benchmark':
        print(s['run'].replace('${{ matrix.topology }}', 'sequence'))
PY
cat > "$T/repo/tests/bench/topology-bench.sh" <<'B'
#!/usr/bin/env bash
for a in "$@"; do :; done
if [ "${TB_REFUSE:-}" = 1 ]; then echo "refused before anything ran" >&2; exit 64; fi
mkdir -p "$TB_OUT"; echo "row ran: $TB_LABEL" >&2; exit 0
B
cat > "$T/repo/tests/bench/minio.sh" <<'M'
#!/usr/bin/env bash
echo "minio.sh $*" >> "$SIM_CALLS"
case $1 in usage) echo "minio_mib 1 objects 1 unfinished_uploads 0" ;; esac
M
chmod +x "$T/repo/tests/bench/"*.sh
# sudo: run the command as is (the root shell sources tb.env and execs env ARGS script).
printf '#!/bin/sh\n[ "$1" = test ] && { shift; exec test "$@"; }\nexec "$@"\n' > "$T/bin/sudo"; chmod +x "$T/bin/sudo"
cd "$T/repo" || exit 2
export SIM_CALLS=$T/calls GITHUB_STEP_SUMMARY=$T/summary RUNNER_TEMP=$T TB_OUT=$T/out TB_WORK=$T/work \
    TB_SEQUENCE=$SEQ TB_MINIO_DIR=$T/minio TB_S3_BUCKET=b PATH="$T/bin:$PATH"
: > "$T/calls"; : > "$T/summary"
rc=0; bash -e "$T/step.sh" > "$T/log" 2>&1 || rc=$?
echo "step rc=$rc"
echo "--- summary"; cat "$T/summary"
echo "--- minio calls"; cat "$T/calls"
echo "--- log tail"; grep -v '^declare -x' "$T/log" | tail -n 8
