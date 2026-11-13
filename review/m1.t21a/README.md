# M1.T21a (skyzfs/skyzfs#48): arm64 runners found out, the unit level on arm64, the package fallback by architecture

`skyzfs-testing-m1.t21` at 5527be7 into `zfs-topology` (merge base 6eeb69b).
Posted as https://github.com/skyzfs/skyzfs/pull/48#issuecomment-6843143979.

**Verdict: nothing blocks. Can merge once the head's runs are green (queued at
the time). One finding is a choice for testing and the lead, three are small.**

Findings:

1. `unit-arm64`, the one arm64 job every pull request runs, is on
   `ubuntu-24.04-arm`, the one image `apt-fallback.sh` does nothing for (no
   mirror list there). An outage like 2026-11-11 fails it at its apt step. Run
   it on `ubuntu-26.04-arm` (fallback shown to work, canary 43359493991), or
   teach the script that image's deb822 file. The step's comment says the
   opposite of what happens.
2. `machine-facts.sh`: the io_uring row needs the kernel's static
   `io_uring_setup` to be a symbol of its own (`-w` does not match
   `__arm64_sys_io_uring_setup`); `mod`'s `/proc/modules` branch compares
   hyphens with underscores; the missing `device/model` prints the shell's
   error; a file system with major 0 gives the number as the device.
3. The `images` inputs: not a defect (matrix only, default on pull requests).

Checked:

- CLAUDE.md's two features (`skyzfsd --features test-hooks`,
  `skyzfs-ublk-sys --features control-test-program`) are the whole set for
  `skyzfs-*`: every use of `test-hooks` in `skyzfs-local` is
  `any(test, feature = "test-hooks")`.
- Run 43356202495 (on 85605ca): `unit-arm64` green in all six cargo steps on
  `ubuntu-24.04-arm`; `lint` and `test` green with the test-hooks steps.
- `test_apt_fallback.sh` passes here; with every architecture given the
  archive, "arm64" and "riscv64" fail.
- The device lookups of `machine-facts.sh` tried on this machine.

For later: the arm64 runners' work disk is the root file system mounted
`nobarrier,data=writeback` with a write-through cache, so a sync is nearly
free; the benchmark round on arm64 will not compare with amd64's on that
account.

Not done: the head's own runs; the probe and canary runs the description
cites; shellcheck; nothing on arm64.
