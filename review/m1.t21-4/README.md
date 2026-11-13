# M1.T21 step 4 (pull request 67), head 56d2638 on 0046598

The benchmark's `runner` input offers the two arm64 images. Review
posted: nothing blocks.

Looked for anything else tied to amd64 in `topology-bench.yml` and
`topology-bench.sh`, and found none:

- all three jobs run on the chosen image; the daemon and every
  `TB_SKYZFSD_REF` build are compiled in the bench job; the artifacts
  between jobs are text; no cache;
- no prebuilt binary is downloaded (Open CAS is cloned and built);
- package names are neutral, kernel packages by `uname -r`;
- the script has no `uname -m`, `/proc/cpuinfo`, page size or
  architecture name; `strace` and `perf trace` take no list of calls.

Not shown: topologies other than the daemon's on arm64 (Open CAS builds
a kernel module). Suggested: the summary names the image and
`uname -m`. The four arm64 runs were not read.

## Second look, head a8ef94d

Posted: can merge once CI is green. The disk-sync line carries `arch`,
`kernel` and `image`; run here with and without the image argument; a
line from before shows "not said" in the table.
