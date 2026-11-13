# From a block trace (ftrace text): for each new ublk device (a read of
# sector 0 by a kworker, the kernel's partition scan), the time from the
# scan to the last request of udev's first burst (its `add` probe), and
# to the first request the tool issued to that device.
/block_rq_issue: 259,/ {
    c = $1; sub(/-[0-9]+$/, "", c)
    for (i = 1; i <= NF; i++) if ($i == "block_rq_issue:") break
    t = $(i-1) + 0; dev = $(i+1); sec = $(i+5)
    if (c ~ /^kworker/ && sec == 0 && $(i+2) == "R") { n++; born[dev] = t; id[dev] = n; last[n] = 0; first[n] = 0; reads[n] = 0; next }
    if (!(dev in id)) next
    k = id[dev]
    if (c == "(udev-worker)" && first[k] == 0) { last[k] = t - born[dev]; reads[k]++ }
    if (c == "skyzfs-blkcheck" && first[k] == 0) first[k] = t - born[dev]
}
END {
    for (k = 1; k <= n; k++) {
        if (first[k] == 0) { none++; continue }
        m++
        gap = first[k] - last[k]
        if (m == 1 || gap < min) min = gap
        if (gap < 0.010) close10++
        if (last[k] > lmax) lmax = last[k]
        r += reads[k]
        if (m == 1 || first[k] < fmin) fmin = first[k]
    }
    printf "devices born: %d; with a request by the tool: %d (none: %d)\n", n, m, none + 0
    printf "udev-worker requests before the tool's first: %d in all, %.0f a device; the last of them at most %.1f ms after the scan\n", r, r / m, lmax * 1000
    printf "the tool's first request: at the earliest %.1f ms after the scan; smallest gap from udev's last request to it %.1f ms; devices with a gap under 10 ms: %d\n", fmin * 1000, min * 1000, close10 + 0
}
