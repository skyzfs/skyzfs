//! Pure, IO-free extent mapping logic (design §4.3).
//!
//! `ExtentMap<T>` is the in-memory overlay used by the engine (design §7.1):
//! a set of non-overlapping `[start, start+len) → T` ranges over a byte
//! space, where absent ranges are *holes* (implicit zero at the volume
//! level). Inserting over existing ranges splits or truncates them —
//! last-writer-wins, exactly the semantics NBD writes need.
//!
//! Verified two ways (design §14.2):
//! - unit tests for each split/truncate shape,
//! - a property test against a flat `Vec<Option<T>>` reference model.
//!
//! TODO(M0→M1): content-addressed COW B+tree nodes over this same
//! semantic (`lookup`/`insert_extent`/`fold_overlay`), then subtree-hash
//! `diff` (design §9.4). The property tests here are reused as the model
//! for those.

use std::collections::BTreeMap;

/// A resolved run returned by [`ExtentMap::query`]: `value == None` is a hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run<T> {
    pub start: u64,
    pub len: u64,
    pub value: Option<T>,
}

/// Non-overlapping interval map with last-writer-wins insertion.
#[derive(Debug, Default, Clone)]
pub struct ExtentMap<T> {
    /// start → (len, value); invariant: entries never overlap.
    inner: BTreeMap<u64, (u64, T)>,
}

impl<T: Clone> ExtentMap<T> {
    pub fn new() -> Self {
        Self {
            inner: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Number of stored extents (fragmentation metric, not byte count).
    pub fn extent_count(&self) -> usize {
        self.inner.len()
    }

    /// Map `[start, start+len)` to `value`, splitting/truncating any
    /// existing overlapping extents. Panics on zero length or overflow —
    /// callers validate via `skynbd_types::Extent` beforehand.
    pub fn insert(&mut self, start: u64, len: u64, value: T) {
        assert!(len > 0, "zero-length insert");
        let end = start.checked_add(len).expect("extent overflows u64");

        // Predecessor that starts before `start` and may reach into it.
        if let Some((&p_start, &(p_len, ref p_val))) = self.inner.range(..start).next_back() {
            let p_end = p_start + p_len;
            if p_end > start {
                let p_val = p_val.clone();
                // Truncate predecessor to [p_start, start).
                self.inner.insert(p_start, (start - p_start, p_val.clone()));
                // If it extended past our end, re-insert its tail.
                if p_end > end {
                    self.inner.insert(end, (p_end - end, p_val));
                }
            }
        }

        // Entries starting inside [start, end): swallowed; keep a tail of the last one.
        let inside: Vec<u64> = self.inner.range(start..end).map(|(&k, _)| k).collect();
        for k in inside {
            let (k_len, k_val) = self.inner.remove(&k).unwrap();
            let k_end = k + k_len;
            if k_end > end {
                self.inner.insert(end, (k_end - end, k_val));
            }
        }

        self.inner.insert(start, (len, value));
    }

    /// Resolve `[start, start+len)` into a complete, ordered cover of runs;
    /// gaps come back as `value: None` holes. The concatenated run lengths
    /// always sum to `len`.
    pub fn query(&self, start: u64, len: u64) -> Vec<Run<T>> {
        assert!(len > 0, "zero-length query");
        let end = start.checked_add(len).expect("query overflows u64");
        let mut runs = Vec::new();
        let mut cursor = start;

        // Predecessor overlapping the query start.
        if let Some((&p_start, &(p_len, ref p_val))) = self.inner.range(..=start).next_back() {
            let p_end = p_start + p_len;
            if p_end > start {
                let run_end = p_end.min(end);
                runs.push(Run {
                    start: cursor,
                    len: run_end - cursor,
                    value: Some(p_val.clone()),
                });
                cursor = run_end;
            }
        }

        let range_from = if cursor == start { start } else { cursor };
        for (&e_start, &(e_len, ref e_val)) in self.inner.range(range_from..end) {
            if e_start >= end {
                break;
            }
            if e_start > cursor {
                runs.push(Run {
                    start: cursor,
                    len: e_start - cursor,
                    value: None,
                });
                cursor = e_start;
            }
            let run_end = (e_start + e_len).min(end);
            runs.push(Run {
                start: cursor,
                len: run_end - cursor,
                value: Some(e_val.clone()),
            });
            cursor = run_end;
        }

        if cursor < end {
            runs.push(Run {
                start: cursor,
                len: end - cursor,
                value: None,
            });
        }
        runs
    }

    /// Iterate all extents in order (for commit building).
    pub fn iter(&self) -> impl Iterator<Item = (u64, u64, &T)> {
        self.inner.iter().map(|(&s, &(l, ref v))| (s, l, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(m: &ExtentMap<u8>, start: u64, len: u64) -> Vec<(u64, u64, Option<u8>)> {
        m.query(start, len)
            .into_iter()
            .map(|r| (r.start, r.len, r.value))
            .collect()
    }

    #[test]
    fn empty_map_is_one_hole() {
        let m: ExtentMap<u8> = ExtentMap::new();
        assert_eq!(runs(&m, 0, 4096), vec![(0, 4096, None)]);
    }

    #[test]
    fn overwrite_middle_splits_into_head_and_tail() {
        let mut m = ExtentMap::new();
        m.insert(0, 4096, b'A');
        m.insert(1024, 512, b'B');
        assert_eq!(
            runs(&m, 0, 4096),
            vec![
                (0, 1024, Some(b'A')),
                (1024, 512, Some(b'B')),
                (1536, 2560, Some(b'A')),
            ]
        );
        assert_eq!(m.extent_count(), 3);
    }

    #[test]
    fn overwrite_spanning_multiple_extents_swallows_them() {
        let mut m = ExtentMap::new();
        m.insert(0, 512, b'A');
        m.insert(512, 512, b'B');
        m.insert(1024, 512, b'C');
        m.insert(256, 1024, b'X'); // covers tail of A, all of B, head of C
        assert_eq!(
            runs(&m, 0, 1536),
            vec![
                (0, 256, Some(b'A')),
                (256, 1024, Some(b'X')),
                (1280, 256, Some(b'C')),
            ]
        );
    }

    #[test]
    fn exact_overwrite_replaces() {
        let mut m = ExtentMap::new();
        m.insert(512, 512, b'A');
        m.insert(512, 512, b'B');
        assert_eq!(runs(&m, 512, 512), vec![(512, 512, Some(b'B'))]);
        assert_eq!(m.extent_count(), 1);
    }

    #[test]
    fn query_covers_gaps_before_between_and_after() {
        let mut m = ExtentMap::new();
        m.insert(1024, 512, b'A');
        m.insert(2048, 512, b'B');
        assert_eq!(
            runs(&m, 0, 3072),
            vec![
                (0, 1024, None),
                (1024, 512, Some(b'A')),
                (1536, 512, None),
                (2048, 512, Some(b'B')),
                (2560, 512, None),
            ]
        );
    }

    #[test]
    fn query_window_truncates_edge_extents() {
        let mut m = ExtentMap::new();
        m.insert(0, 2048, b'A');
        assert_eq!(runs(&m, 512, 1024), vec![(512, 1024, Some(b'A'))]);
    }
}

#[cfg(test)]
mod model_tests {
    //! Property test vs a flat reference model (design §14.2, layer 2):
    //! random insert sequences must make `ExtentMap` and a plain
    //! byte-array model agree exactly, byte for byte.

    use super::*;
    use proptest::prelude::*;

    const SPACE: u64 = 64 * 512; // small address space to force overlaps

    fn apply_model(model: &mut [Option<u8>], start: u64, len: u64, v: u8) {
        for b in &mut model[start as usize..(start + len) as usize] {
            *b = Some(v);
        }
    }

    fn materialize(m: &ExtentMap<u8>) -> Vec<Option<u8>> {
        let mut out = vec![None; SPACE as usize];
        for r in m.query(0, SPACE) {
            if let Some(v) = r.value {
                for b in &mut out[r.start as usize..(r.start + r.len) as usize] {
                    *b = Some(v);
                }
            }
        }
        out
    }

    fn op_strategy() -> impl Strategy<Value = (u64, u64, u8)> {
        // 512-aligned start/len within SPACE, len >= 512.
        (0..SPACE / 512, 1..=8u64, any::<u8>()).prop_filter_map("fits in space", |(s, l, v)| {
            let start = s * 512;
            let len = l * 512;
            (start + len <= SPACE).then_some((start, len, v))
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn map_matches_flat_model(ops in proptest::collection::vec(op_strategy(), 0..64)) {
            let mut map = ExtentMap::new();
            let mut model = vec![None; SPACE as usize];
            for (start, len, v) in ops {
                map.insert(start, len, v);
                apply_model(&mut model, start, len, v);
            }
            prop_assert_eq!(materialize(&map), model);
        }

        #[test]
        fn query_is_a_complete_ordered_cover(ops in proptest::collection::vec(op_strategy(), 0..32),
                                             win in op_strategy()) {
            let mut map = ExtentMap::new();
            for (start, len, v) in ops {
                map.insert(start, len, v);
            }
            let (qs, ql, _) = win;
            let runs = map.query(qs, ql);
            let mut cursor = qs;
            for r in &runs {
                prop_assert_eq!(r.start, cursor, "runs must be contiguous and ordered");
                prop_assert!(r.len > 0);
                cursor += r.len;
            }
            prop_assert_eq!(cursor, qs + ql, "runs must cover the whole window");
        }
    }
}
