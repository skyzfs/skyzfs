# shellcheck shell=bash
# has_request_log SERVER: 0 when the server's build keeps a request log
# (`serve --request-log PATH`, the daemon's test build from M1.3y on).
# The hooked daemon prints its hooks after the usage and exits 2, and
# the stand-in prints its usage and exits 2: the text is read whole,
# since under pipefail a pipe into grep would fail on that status.
has_request_log() { local h; h=$("$1" serve --help 2>&1); [[ $h == *--request-log* ]]; }
