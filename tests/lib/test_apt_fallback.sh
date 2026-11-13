#!/bin/bash
# Tests of tests/lib/apt-fallback.sh, with no network and no root.
# Run: bash tests/lib/test_apt_fallback.sh
HERE=$(cd "$(dirname "$0")" && pwd)
S=$HERE/apt-fallback.sh
# shellcheck source=tests/lib/apt-fallback.sh
. "$S"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
is() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi; }
has() { case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' is not in '$2'"; fails=$((fails + 1)) ;; esac; }

# The list as it is on the runner images (run 43060459690), tabs and all.
image_list() { printf 'http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\nhttps://archive.ubuntu.com/ubuntu/\tpriority:2\nhttps://security.ubuntu.com/ubuntu/\tpriority:3\n' > "$1"; }
image_list "$T/list"
is "the hosts of the image's list, in order" "$(mirror_hosts "$T/list" | tr '\n' ' ')" "azure.archive.ubuntu.com archive.ubuntu.com security.ubuntu.com "
printf '# a comment\n\n  \nhttps://a.example:8443/ubuntu/ priority:1\nmirror.example/ubuntu\nhttp://b.example\n' > "$T/odd"
is "comments, empty lines, a port, a line that is no URL, a URL with no path" "$(mirror_hosts "$T/odd" | tr '\n' ' ')" "a.example b.example "

# Lookups for the runs below: a name resolves unless it is in DEAD.
cat > "$T/lookup" <<'LOOKUP'
#!/bin/bash
echo "$1" >> "$LOOKUPS"
case " $DEAD " in *" $1 "*) exit 1 ;; esac
exit 0
LOOKUP
chmod +x "$T/lookup"
run() { # DEAD-NAMES: run the script on $T/list; RC, OUT and SUMMARY are set
    : > "$T/summary"; : > "$T/lookups"
    OUT=$(DEAD="$1" LOOKUPS="$T/lookups" APT_FALLBACK_LOOKUP="$T/lookup" APT_MIRROR_LIST="$T/list" APT_SOURCES="$T/sources.d" \
        APT_FALLBACK_MIRROR="${MIRROR:-https://mirror.example/ubuntu/}" GITHUB_STEP_SUMMARY="$T/summary" GITHUB_ACTIONS="" \
        RUNNER_ENVIRONMENT="${RUNNER_ENV:-}" bash "$S" 2>&1); RC=$?
    SUMMARY=$(cat "$T/summary")
}

# The sources as on the images: one file, every suite through the list.
mkdir "$T/sources.d"
printf 'Types: deb\nURIs: mirror+file:%s\nSuites: noble noble-updates\n' "$T/list" > "$T/sources.d/ubuntu.sources"
image_list "$T/list"; cp "$T/list" "$T/before"
run ""
is "all resolve: exit 0" "$RC" 0
is "all resolve: the list is as it was" "$(cmp -s "$T/list" "$T/before" && echo same)" same
is "all resolve: nothing in the summary" "$SUMMARY" ""
has "all resolve: the log says so" "$OUT" "nothing changed"

run "archive.ubuntu.com security.ubuntu.com azure.archive.ubuntu.com"
is "none resolves: exit 0" "$RC" 0
is "none resolves: the list holds the fallback alone" "$(cat "$T/list")" "$(printf 'https://mirror.example/ubuntu/\tpriority:1')"
is "none resolves: the image's list is kept beside it" "$(cmp -s "$T/list.image" "$T/before" && echo same)" same
has "none resolves: the summary says the fallback is in use" "$SUMMARY" "the fallback is in use"
has "none resolves: and names the mirror" "$SUMMARY" "https://mirror.example/ubuntu/"
has "none resolves: and names what did not resolve" "$SUMMARY" "azure.archive.ubuntu.com"

run "archive.ubuntu.com"
has "a second run finds the fallback in place and changes nothing" "$OUT" "already holds the fallback"
is "a second run looks nothing up" "$(wc -l < "$T/lookups" | tr -d ' ')" 0
is "a second run keeps the image's list as saved" "$(cmp -s "$T/list.image" "$T/before" && echo same)" same

is "the list that was put in place is readable by all" "$(stat -c %a "$T/list")" 644
is "no half-made list is left beside it" "$([ -e "$T/list.new" ] && echo left)" ""

image_list "$T/list"
RUNNER_ENV=self-hosted run "archive.ubuntu.com security.ubuntu.com azure.archive.ubuntu.com"
is "a self-hosted runner: the list is as it was" "$(cmp -s "$T/list" "$T/before" && echo same)" same
has "a self-hosted runner: says so" "$OUT" "not github-hosted"
is "a self-hosted runner: looks nothing up" "$(wc -l < "$T/lookups" | tr -d ' ')" 0
RUNNER_ENV=github-hosted run "archive.ubuntu.com security.ubuntu.com azure.archive.ubuntu.com"
is "a github-hosted runner: the fallback alone" "$(mirror_hosts "$T/list")" "mirror.example"

# The directory cannot be written and there is no sudo that helps: the
# list stays the image's, whole, and the message says so.
image_list "$T/list"; mkdir "$T/bin"; printf '#!/bin/sh\nexit 1\n' > "$T/bin/sudo"; chmod +x "$T/bin/sudo"
chmod a-w "$T"
if [ -w "$T" ]; then
    echo "skip the list cannot be replaced: this user (root) can write anywhere"
else
    PATH="$T/bin:$PATH" run "archive.ubuntu.com"
    is "the list cannot be replaced: exit 0" "$RC" 0
    is "the list cannot be replaced: it is the image's, whole" "$(cmp -s "$T/list" "$T/before" && echo same)" same
    has "the list cannot be replaced: the summary does not claim the fallback" "$SUMMARY" "Nothing changed"
fi
chmod u+w "$T"

image_list "$T/list"
run "security.ubuntu.com"
is "one of three does not resolve: the fallback alone" "$(mirror_hosts "$T/list")" "mirror.example"

image_list "$T/list"
run "archive.ubuntu.com security.ubuntu.com azure.archive.ubuntu.com mirror.example"
is "the fallback does not resolve either: exit 0" "$RC" 0
is "the fallback does not resolve either: the list is as it was" "$(cmp -s "$T/list" "$T/before" && echo same)" same
has "the fallback does not resolve either: the summary says so" "$SUMMARY" "neither did the fallback"

image_list "$T/list"
MIRROR="http://mirror.example/ubuntu/" run "archive.ubuntu.com"
is "a fallback that is not https is refused" "$RC" 64
is "a fallback that is not https: the list is as it was" "$(cmp -s "$T/list" "$T/before" && echo same)" same

printf 'Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: noble\n' > "$T/sources.d/ubuntu.sources"
run "archive.ubuntu.com security.ubuntu.com azure.archive.ubuntu.com"
is "no source uses the list: the list is as it was" "$(cmp -s "$T/list" "$T/before" && echo same)" same
has "no source uses the list: says so" "$OUT" "no apt source uses"
is "no source uses the list: looks nothing up" "$(wc -l < "$T/lookups" | tr -d ' ')" 0

rm -f "$T/list"
run "archive.ubuntu.com"
is "no list on the machine: exit 0" "$RC" 0
has "no list on the machine: says so" "$OUT" "nothing changed"

# The script loosens nothing: no option of apt, no key.
is "no option that switches a check off, no key" \
    "$(grep -v '^#' "$S" | grep -c -i -E 'trusted=|allow-unauth|AllowInsecure|AllowUnauth|apt-key|\.gpg|signed-by|apt\.conf')" 0

# The fallback's tree follows the machine's architecture: arm64 packages
# are only in ubuntu-ports. A fallback named by hand is kept as given.
# shellcheck source=tests/lib/apt-fallback.sh
fallback_for() { ( unset APT_FALLBACK_MIRROR; APT_FALLBACK_ARCH=$1; . "$S"; echo "$APT_FALLBACK_MIRROR" ); }
is "amd64: the archive" "$(fallback_for amd64)" "https://mirrors.edge.kernel.org/ubuntu/"
is "i386: the archive" "$(fallback_for i386)" "https://mirrors.edge.kernel.org/ubuntu/"
is "arm64: the ports tree" "$(fallback_for arm64)" "https://mirrors.edge.kernel.org/ubuntu-ports/"
is "riscv64: the ports tree" "$(fallback_for riscv64)" "https://mirrors.edge.kernel.org/ubuntu-ports/"
# shellcheck source=tests/lib/apt-fallback.sh
is "a fallback named by hand is kept" "$( ( APT_FALLBACK_MIRROR=https://m.example/ubuntu-ports/ APT_FALLBACK_ARCH=amd64; . "$S"; echo "$APT_FALLBACK_MIRROR" ) )" "https://m.example/ubuntu-ports/"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"
