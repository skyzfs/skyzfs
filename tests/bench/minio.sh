#!/usr/bin/env bash
# MinIO on the runner for the benchmark's uploads (TB_SKZ_UPLOAD, M2.4c).
#
#   tests/bench/minio.sh up DIR BUCKET    start it on DIR, make BUCKET
#   tests/bench/minio.sh usage DIR BUCKET one line: MiB on DIR, objects
#                                         in BUCKET, uploads not finished
#   tests/bench/minio.sh down DIR         stop it and delete DIR (named
#                                         minio-data, or exit 64)
#
# The data is on DIR, a directory of the runner, so that what a row
# left there is measured from outside the container and is gone with
# DIR. Each row's volume is a new one, and an upload that is abandoned
# before its commit leaves what it had PUT: the workflow records each
# upload row's usage and starts MinIO anew after it, or the rows fill
# the disk (run 44325181328: rows 7 to 9 refused for want of space).
# The keys are MinIO's local defaults, not secrets. pgsty/minio is the
# community fork, pinned as in the rigs (minio/minio left Docker Hub in
# Sept 2026). The bucket is made with curl's SigV4, so nothing else is
# built. Exit 2 when MinIO does not come up or the bucket is refused.
set -euo pipefail

IMAGE=pgsty/minio:RELEASE.2026-08-04T00-00-00Z
ENDPOINT=http://127.0.0.1:9000

up() { # dir bucket
    local dir=$1 bucket=$2 code
    sudo mkdir -p "$dir"
    docker run -d --name minio -p 9000:9000 -v "$dir:/data" \
        -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
        "$IMAGE" server /data > /dev/null
    for _ in $(seq 1 60); do
        if curl -sf "$ENDPOINT/minio/health/live"; then
            code=$(curl -s -o /dev/stderr -w '%{http_code}' -X PUT --user minioadmin:minioadmin \
                --aws-sigv4 "aws:amz:us-east-1:s3" "$ENDPOINT/$bucket")
            [ "$code" = 200 ] || { echo "making the bucket $bucket answered $code" >&2; exit 2; }
            return 0
        fi
        sleep 1
    done
    echo "MinIO did not answer in 60 s" >&2
    docker logs minio 2>&1 | tail -n 20 >&2
    exit 2
}

usage() { # dir bucket
    local dir=$1 bucket=$2 mib objects pending
    mib=$(sudo du -sm "$dir" | cut -f1)
    objects=$(sudo find "$dir/$bucket" -name xl.meta 2> /dev/null | wc -l)
    pending=$(sudo find "$dir/.minio.sys/multipart" -mindepth 2 -maxdepth 2 -type d 2> /dev/null | wc -l)
    echo "minio_mib $mib objects $objects unfinished_uploads $pending"
}

down() { # dir
    # Deleted as root: only a directory named minio-data, as the
    # workflow names it, so that a wrong argument deletes nothing else.
    [ "$(basename -- "${1:?}")" = minio-data ] || { echo "minio.sh down deletes only a directory named minio-data, not: $1" >&2; exit 64; }
    docker rm -f minio > /dev/null 2>&1 || true
    sudo rm -rf -- "${1:?}"
}

case "${1:-}" in
    up) up "${2:?dir}" "${3:?bucket}" ;;
    usage) usage "${2:?dir}" "${3:?bucket}" ;;
    down) down "${2:?dir}" ;;
    *) echo "usage: $0 up DIR BUCKET | usage DIR BUCKET | down DIR" >&2; exit 64 ;;
esac
