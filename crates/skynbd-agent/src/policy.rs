//! Reclaim policy (design doc §7, §7.1): which executor runs an advance's
//! reclaim, and when reclaim happens at all.
//!
//! The two executors — discard port (minor compaction) and two-pool swing
//! (major compaction) — satisfy one contract; the §3 sequence is agnostic
//! between them and the choice is never durable ("the strategy choice
//! itself is never part of the durable contract — only the slot state
//! is"). [`decide`] is the §7 comparison, pure over data the engine
//! already holds after the enumeration, so a crashed swing simply
//! re-decides on retry.

use std::str::FromStr;

/// Per-StorageClass executor override (§7): `auto` runs the §7
/// comparison; `port`/`swing` force one executor — except that a `swing`
/// override still yields to the headroom guard (it cannot force an
/// unstartable swing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReclaimStrategy {
    #[default]
    Auto,
    Port,
    Swing,
}

impl FromStr for ReclaimStrategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "port" => Ok(Self::Port),
            "swing" => Ok(Self::Swing),
            other => Err(format!("unknown reclaimStrategy {other:?}")),
        }
    }
}

/// §7.1: reclaim timing is cache eviction policy, orthogonal to the
/// executor. `eager` reclaims at every advance (pool = unpublished
/// delta); `lazy` retains the published set as read cache and evicts
/// only under watermark pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReclaimTiming {
    #[default]
    Eager,
    Lazy,
}

impl FromStr for ReclaimTiming {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "eager" => Ok(Self::Eager),
            "lazy" => Ok(Self::Lazy),
            other => Err(format!("unknown reclaimTiming {other:?}")),
        }
    }
}

/// The §7 compaction-policy dials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimPolicy {
    pub strategy: ReclaimStrategy,
    pub timing: ReclaimTiming,
    /// `reclaimRangeThreshold`: below this many shared ranges the port's
    /// per-range frozen cost beats the swing's fixed window. Calibrated
    /// per rig from the S15b timing table; the default is the measured
    /// PoC crossover (R ≈ 2k).
    pub range_threshold: usize,
    /// `swingEveryNAdvances` (§7 "aged"): force a major compaction every
    /// N advances so a volume with perpetually small deltas still gets
    /// its pool reset. `None` = never age.
    pub swing_every_n: Option<u32>,
}

impl Default for ReclaimPolicy {
    fn default() -> Self {
        Self {
            strategy: ReclaimStrategy::Auto,
            timing: ReclaimTiming::Eager,
            range_threshold: 2048,
            swing_every_n: None,
        }
    }
}

/// The executor an advance's reclaim runs through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorChoice {
    /// R == 0: nothing shared, nothing to reclaim (idle volume — the
    /// common case).
    NoOp,
    Port,
    Swing,
}

/// The §7 decision, one comparison over data already in hand:
///
/// ```text
/// R == 0                                  → no-op
/// strategy == port, or no swing on hand   → port
/// headroom unknown or short of D + slot   → port (forced — the guard
///   transient                               overrides even `swing`)
/// strategy == swing                       → swing
/// aged (advances_since_swing ≥ N)         → swing
/// R ≤ reclaimRangeThreshold               → port
/// otherwise                               → swing
/// ```
///
/// `headroom` is `(fs_free, required_transient)` from the stack —
/// `None` means the stack cannot answer, and unknown is conservative:
/// swing cost scales with D plus transient space for D and slot
/// metadata, and choosing it blind walks into §8.1's exhaustion ladder.
pub fn decide(
    shared_count: usize,
    dirty_bytes: u64,
    headroom: Option<(u64, u64)>,
    swing_supported: bool,
    degraded: bool,
    advances_since_swing: u32,
    p: &ReclaimPolicy,
) -> ExecutorChoice {
    if shared_count == 0 {
        return ExecutorChoice::NoOp;
    }
    // §8.2 step 5: a volume §8.1 rule 3 has taken read-only reclaims
    // with the PORT executor, whatever the policy says. The reason is
    // that the port drains ALLOCATION-FREE, which is what lets the
    // runbook run it against a pool with nothing left to give.
    //
    // The headroom guard below does not cover this: it measures the
    // backing filesystem under the slot file, not the pool, so a
    // degraded volume on a roomy node filesystem would still pick Swing
    // and then try to build a second pool while the first is full.
    //
    // An earlier draft gave a second reason — that the swing's frozen
    // top-up would fail because a read-only superblock answers EROFS to
    // `FIFREEZE`. That is false (a clean read-only super freezes
    // successfully, measured), and it is worth recording WHY it had to
    // be false: the port freezes too, in `reclaim_window` and `evict`,
    // and neither handles `Wedged` — deliberately, since only `capture`
    // must proceed on a wedged filesystem. Had a degraded volume really
    // been unfreezable, routing it to the port would have failed its
    // reclaim, left `freed_this_cycle` unset, and stranded the volume
    // read-only for good. The rule survives on the allocation argument
    // alone; it never rested on the errno.
    if p.strategy == ReclaimStrategy::Port || !swing_supported || degraded {
        return ExecutorChoice::Port;
    }
    match headroom {
        Some((fs_free, required)) if fs_free >= dirty_bytes.saturating_add(required) => {}
        // Unknown or short: §8.1's "a swing cannot start without
        // headroom" — this also overrides `strategy == Swing`.
        _ => return ExecutorChoice::Port,
    }
    if p.strategy == ReclaimStrategy::Swing {
        return ExecutorChoice::Swing;
    }
    if p.swing_every_n.is_some_and(|n| advances_since_swing >= n) {
        return ExecutorChoice::Swing;
    }
    if shared_count <= p.range_threshold {
        return ExecutorChoice::Port;
    }
    ExecutorChoice::Swing
}

#[cfg(test)]
mod tests {
    use super::*;
    use ExecutorChoice::*;

    fn p(strategy: ReclaimStrategy, threshold: usize, every_n: Option<u32>) -> ReclaimPolicy {
        ReclaimPolicy {
            strategy,
            timing: ReclaimTiming::Eager,
            range_threshold: threshold,
            swing_every_n: every_n,
        }
    }

    const ROOM: Option<(u64, u64)> = Some((1 << 40, 1 << 20));

    /// [`super::decide`] with §8.2 step 5's `degraded` term off. Every
    /// row in this module predates rule 3 and means "a healthy volume";
    /// the degraded rows below call `super::decide` directly so the
    /// term they exercise is visible at the call.
    fn decide(
        shared_count: usize,
        dirty_bytes: u64,
        headroom: Option<(u64, u64)>,
        swing_supported: bool,
        advances_since_swing: u32,
        p: &ReclaimPolicy,
    ) -> ExecutorChoice {
        super::decide(
            shared_count,
            dirty_bytes,
            headroom,
            swing_supported,
            false,
            advances_since_swing,
            p,
        )
    }

    #[test]
    fn degraded_volume_always_ports() {
        // §8.2 step 5. The strongest form: a `Swing` override, a
        // supporting stack, ample headroom and an over-threshold set —
        // every input that would otherwise force a swing — loses to
        // the degraded term. The frozen top-up a swing needs cannot
        // run on the read-only superblock rule 3 just created.
        assert_eq!(
            super::decide(
                9999,
                0,
                ROOM,
                true,
                true,
                99,
                &p(ReclaimStrategy::Swing, 0, Some(1))
            ),
            Port
        );
        // And it is genuinely the degraded term doing it: the same row
        // undegraded swings.
        assert_eq!(
            super::decide(
                9999,
                0,
                ROOM,
                true,
                false,
                99,
                &p(ReclaimStrategy::Swing, 0, Some(1))
            ),
            Swing
        );
        // An empty reclaim set still short-circuits first: there is
        // nothing to port, and inventing work for a degraded volume is
        // the opposite of the intent.
        assert_eq!(
            super::decide(
                0,
                0,
                ROOM,
                true,
                true,
                99,
                &p(ReclaimStrategy::Auto, 0, None)
            ),
            NoOp
        );
    }

    #[test]
    fn empty_set_is_noop_regardless_of_everything_else() {
        for strategy in [
            ReclaimStrategy::Auto,
            ReclaimStrategy::Port,
            ReclaimStrategy::Swing,
        ] {
            assert_eq!(
                decide(0, 1 << 30, None, true, 99, &p(strategy, 0, Some(1))),
                NoOp
            );
        }
    }

    #[test]
    fn port_override_always_ports() {
        assert_eq!(
            decide(
                9999,
                0,
                ROOM,
                true,
                99,
                &p(ReclaimStrategy::Port, 0, Some(1))
            ),
            Port
        );
    }

    #[test]
    fn unsupported_stack_always_ports() {
        assert_eq!(
            decide(
                9999,
                0,
                ROOM,
                false,
                99,
                &p(ReclaimStrategy::Swing, 0, Some(1))
            ),
            Port
        );
        assert_eq!(
            decide(9999, 0, ROOM, false, 99, &p(ReclaimStrategy::Auto, 0, None)),
            Port
        );
    }

    #[test]
    fn unknown_headroom_is_conservative() {
        assert_eq!(
            decide(9999, 0, None, true, 0, &p(ReclaimStrategy::Auto, 0, None)),
            Port
        );
    }

    #[test]
    fn short_headroom_forces_port_even_over_a_swing_override() {
        // fs_free < dirty + required: the §8.1 guard wins over `swing`.
        let short = Some((100, 60));
        assert_eq!(
            decide(
                9999,
                50,
                short,
                true,
                0,
                &p(ReclaimStrategy::Swing, 0, None)
            ),
            Port
        );
        // Exactly enough starts the swing.
        let exact = Some((110, 60));
        assert_eq!(
            decide(
                9999,
                50,
                exact,
                true,
                0,
                &p(ReclaimStrategy::Swing, 0, None)
            ),
            Swing
        );
    }

    #[test]
    fn swing_override_swings_when_startable() {
        // Even under the threshold and un-aged.
        assert_eq!(
            decide(1, 0, ROOM, true, 0, &p(ReclaimStrategy::Swing, 2048, None)),
            Swing
        );
    }

    #[test]
    fn aged_volume_swings_under_auto() {
        let policy = p(ReclaimStrategy::Auto, 2048, Some(4));
        assert_eq!(decide(1, 0, ROOM, true, 3, &policy), Port, "not aged yet");
        assert_eq!(decide(1, 0, ROOM, true, 4, &policy), Swing, "aged");
        // No cadence configured: never ages.
        assert_eq!(
            decide(
                1,
                0,
                ROOM,
                true,
                u32::MAX,
                &p(ReclaimStrategy::Auto, 2048, None)
            ),
            Port
        );
    }

    #[test]
    fn auto_crosses_over_at_the_range_threshold() {
        let policy = p(ReclaimStrategy::Auto, 100, None);
        assert_eq!(decide(100, 0, ROOM, true, 0, &policy), Port, "at threshold");
        assert_eq!(
            decide(101, 0, ROOM, true, 0, &policy),
            Swing,
            "past threshold"
        );
    }

    #[test]
    fn dial_strings_parse() {
        assert_eq!("auto".parse(), Ok(ReclaimStrategy::Auto));
        assert_eq!("port".parse(), Ok(ReclaimStrategy::Port));
        assert_eq!("swing".parse(), Ok(ReclaimStrategy::Swing));
        assert!("Swing".parse::<ReclaimStrategy>().is_err());
        assert_eq!("eager".parse(), Ok(ReclaimTiming::Eager));
        assert_eq!("lazy".parse(), Ok(ReclaimTiming::Lazy));
        assert!("never".parse::<ReclaimTiming>().is_err());
    }
}
