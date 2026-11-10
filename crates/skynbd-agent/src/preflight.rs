//! Placement preflight (V3.2 §9.3 requirement: validate *before* the
//! first capture, never during one; agent design doc §6.3).
//!
//! With `DmStack` there is no lvm.conf hazard class — the remaining rules
//! are: agent persistence must not live on a freezable filesystem, and
//! the pool's chunk size must match the configured object chunk.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreflightReport {
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    /// Posture the operator has to know but that no rule can decide for
    /// them — the §8.1 `no_space_timeout` framing, per rig.
    pub notes: Vec<String>,
}

impl PreflightReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Pure device-identity check: `path` must not reside on the same
/// filesystem (`st_dev`) as any freezable mountpoint.
pub fn check_placement(
    state_dir: &Path,
    mountpoints: &[&Path],
) -> Result<PreflightReport, std::io::Error> {
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    std::fs::create_dir_all(state_dir)?;
    let sd = std::fs::metadata(state_dir)?.dev();
    for m in mountpoints {
        match std::fs::metadata(m) {
            Ok(md) if md.dev() == sd => failures.push(format!(
                "state dir {} is on the freezable filesystem {} (st_dev {:#x}) — \
                 a frozen fs would deadlock the agent (V3.2 §9.3)",
                state_dir.display(),
                m.display(),
                sd
            )),
            Ok(_) => {}
            Err(e) => warnings.push(format!("mountpoint {}: {e}", m.display())),
        }
    }
    Ok(PreflightReport {
        failures,
        warnings,
        notes: vec![],
    })
}

/// Chunk divisor rule: object chunk must equal the pool's data block size
/// (MVP; `N ×` block support is post-MVP per V3.2 §21.4).
pub fn check_chunk(pool_block_bytes: u64, object_chunk_bytes: u64) -> PreflightReport {
    let mut failures = Vec::new();
    if pool_block_bytes != object_chunk_bytes {
        failures.push(format!(
            "pool data block size {pool_block_bytes} != object chunk {object_chunk_bytes} \
             (MVP requires equality; V3.2 §10.3)"
        ));
    }
    PreflightReport {
        failures,
        warnings: vec![],
        notes: vec![],
    }
}

/// Where the kernel publishes dm-thin's queue-mode bound.
const NO_SPACE_TIMEOUT: &str = "/sys/module/dm_thin_pool/parameters/no_space_timeout";

/// §8.1 exhaustion-ladder posture: report `no_space_timeout`, the
/// module parameter that bounds **queue mode** — the middle rung, where
/// provisioning writes hang instead of erroring. Its value decides what
/// a too-late watermark costs: the ladder's next rung is
/// out-of-data-space mode, whose `BLK_STS_NOSPC` puts ext4 in
/// remount-ro and shuts XFS down, and that state outlives the shortage.
///
/// `0` means queue forever: writes hang indefinitely rather than error,
/// which trades a shut-down filesystem for a hung one. That is a
/// deliberate posture and never a *failure* — but it is also the
/// largest value the knob can take, so it warns like every other value
/// that can outlast the freeze budget. Reporting the queue-forever
/// posture without the warning had it silent at exactly the setting
/// that exceeds the budget by the widest margin.
///
/// The warning threshold is "can the timeout outlast the freeze budget
/// **at all**", not some multiple of it. The failure mode is not
/// gradual: the rescue advance opens with an fsfreeze, a freeze behind
/// queued no-space writes cannot complete until the queue drains or
/// errors, and the watchdog force-thaws at the budget. A 6 s timeout
/// against a 5 s budget already loses the race — §8.1 rule 1's failure
/// mode seen from the kernel's side — and warning only at 2× left that
/// whole band silent.
///
/// An absent file is not an error: `dm_thin_pool` may be built in or
/// not yet loaded, and neither says anything about the posture.
pub fn check_no_space_timeout(freeze_budget_ms: u64) -> PreflightReport {
    check_no_space_timeout_at(Path::new(NO_SPACE_TIMEOUT), freeze_budget_ms)
}

fn check_no_space_timeout_at(path: &Path, freeze_budget_ms: u64) -> PreflightReport {
    let mut r = PreflightReport::default();
    let Ok(raw) = std::fs::read_to_string(path) else {
        r.notes.push(format!(
            "dm-thin no_space_timeout: unknown ({} unreadable — module built in or \
             not loaded); queue mode's bound is whatever this kernel defaults to \
             (60 s upstream) (§8.1)",
            path.display()
        ));
        return r;
    };
    let Ok(secs) = raw.trim().parse::<u64>() else {
        r.warnings.push(format!(
            "dm-thin no_space_timeout: unparsable {raw:?} in {}",
            path.display()
        ));
        return r;
    };
    if secs == 0 {
        r.notes.push(
            "dm-thin no_space_timeout: 0 — the pool QUEUES FOREVER on exhaustion: \
             provisioning writes hang indefinitely and never reach out-of-data-space \
             mode. No dirty shutdown, no bound on the hang either (§8.1)"
                .to_string(),
        );
        r.warnings.push(format!(
            "dm-thin no_space_timeout is 0 (queue forever) against a {freeze_budget_ms} ms \
             freeze budget: an unbounded hang outlasts every budget, so a capture that \
             starts after the pool has entered queue mode is force-thawed by the watchdog \
             and the rescue advance never completes (§8.1 rule 1)"
        ));
        return r;
    }
    r.notes.push(format!(
        "dm-thin no_space_timeout: {secs} s — queue mode hangs provisioning writes \
         that long, then out-of-data-space mode errors them (ext4 remount-ro, XFS \
         shutdown, outliving the shortage) (§8.1)"
    ));
    // The rescue advance opens with a freeze, and a freeze behind queued
    // no-space writes is exactly what the watchdog kills — so the line
    // is where the hang can reach the budget at all, not a multiple of
    // it.
    if secs.saturating_mul(1000) >= freeze_budget_ms {
        r.warnings.push(format!(
            "dm-thin no_space_timeout ({secs} s) can outlast the freeze budget \
             ({freeze_budget_ms} ms): a capture that starts after the pool has \
             entered queue mode is held past the watchdog, force-thawing the \
             rescue advance instead of completing it (§8.1 rule 1)"
        ));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_device_placement_fails() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        // The temp dir and itself are trivially the same st_dev.
        let r = check_placement(&state, &[dir.path()]).unwrap();
        assert!(!r.ok());
        assert!(r.failures[0].contains("freezable"));
        // No mountpoints ⇒ trivially fine.
        let r = check_placement(&state, &[]).unwrap();
        assert!(r.ok());
    }

    #[test]
    fn chunk_rule() {
        assert!(check_chunk(65536, 65536).ok());
        assert!(!check_chunk(131072, 65536).ok());
    }

    /// §8.1 posture: the three shapes the knob can take, none of them a
    /// failure — the parameter is the operator's call, and preflight's
    /// job is to say what it costs.
    #[test]
    fn no_space_timeout_posture() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("no_space_timeout");

        // Absent: module built in or not loaded — unknown, not a fault.
        let r = check_no_space_timeout_at(&p, 5_000);
        assert!(r.ok() && r.warnings.is_empty());
        assert!(r.notes[0].contains("unknown"), "{:?}", r.notes);

        // Queue-forever: still never a failure, but the unbounded hang
        // exceeds every budget, so it is the last value that should go
        // unwarned.
        std::fs::write(&p, "0\n").unwrap();
        let r = check_no_space_timeout_at(&p, 5_000);
        assert!(r.ok(), "posture is never a failure");
        assert!(r.notes[0].contains("QUEUES FOREVER"), "{:?}", r.notes);
        assert!(r.warnings[0].contains("queue forever"), "{:?}", r.warnings);

        // The upstream default against a 5 s freeze budget: reported,
        // and warned about — 60 s of hung writes outlasts the watchdog.
        std::fs::write(&p, "60\n").unwrap();
        let r = check_no_space_timeout_at(&p, 5_000);
        assert!(r.ok(), "posture is never a failure");
        assert!(r.notes[0].contains("60 s"), "{:?}", r.notes);
        assert!(r.warnings[0].contains("freeze budget"), "{:?}", r.warnings);

        // The band the 2×-budget threshold used to miss: 6 s of queued
        // writes against a 5 s budget already outlasts the watchdog.
        std::fs::write(&p, "6\n").unwrap();
        let r = check_no_space_timeout_at(&p, 5_000);
        assert!(
            r.warnings[0].contains("can outlast the freeze budget"),
            "{:?}",
            r.warnings
        );

        // A budget the timeout cannot reach: reported, not warned.
        let r = check_no_space_timeout_at(&p, 60_000);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.notes.len(), 1);
    }
}
