//! The check before each cut (the lead's Q1, Q5 and Q6 of M2.4b, decided
//! 2026-11-17): what head's row and head's commit say of the cut being
//! uploaded.

use crate::local::Uploadable;
use crate::row::Row;

/// How a cut is uploaded, which the status shows for each (Q6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The cut holds what changed since the cut the intent map names as
    /// committed, and is uploaded onto it: the row names that cut.
    Delta,
    /// The cut holds the whole volume: the first of a cut state begun
    /// with nothing known, or the first after a mismatch. It needs no
    /// base.
    Whole,
}

/// Why the row or head does not hold what the held cuts were made
/// against, which the status shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// A cut that holds part of the volume, and no row: nothing it could
    /// be a change of is in object storage.
    NoRow,
    /// The row names another cut than the one the intent map names as
    /// committed: another state's, or another number of the same state.
    /// Among these, a cut of the same state with a higher number than the
    /// map's (left only by a power cut between a commit and the map's
    /// header reaching the disk) is a mismatch too (the lead's Q8, open).
    NotTheBase,
    /// Head's commit is not the row's: its group is another, or it has
    /// one where there is no row. Something other than this uploader moved
    /// head (an operator's rollback, another writer), and the uploader does
    /// not move it on its own (Q5).
    HeadMoved,
    /// The row cannot be read (`row::RowError`): it is not overwritten.
    Unreadable,
}

impl Mismatch {
    /// The reason in a few words, for the status.
    pub fn said(self) -> &'static str {
        match self {
            Mismatch::NoRow => "no row",
            Mismatch::NotTheBase => "the row names another cut",
            Mismatch::HeadMoved => "head's commit is not the row's",
            Mismatch::Unreadable => "the row cannot be read",
        }
    }
}

/// What the check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The row names the cut being uploaded: its commit had landed before
    /// the daemon let it go. Let it go, and upload nothing.
    Committed,
    /// Upload the cut, by this rule.
    Upload(Rule),
    /// Abandon every held cut, so that the next cut holds the whole
    /// volume.
    Mismatch(Mismatch),
}

/// The check of `cut`, the cut being uploaded, against head's row as
/// read and the group of head's commit (`None` for a commit with no
/// group), in this order:
///
/// 1. the row names the cut (its state, number and group): it is
///    committed;
/// 2. a cut that holds the whole volume is uploaded when head's commit is
///    the row's (its group is the row's), whatever cut the row names, or,
///    with no row, when head's commit has no group;
/// 3. a cut that holds part of the volume is uploaded when the row names
///    the cut the intent map names as committed, and head's commit is the
///    row's;
/// 4. else a mismatch, and a row that cannot be read is always one.
pub fn judge(cut: &Uploadable, row: &Row, head_group: Option<u64>) -> Verdict {
    let named = match row {
        Row::Unreadable(_) => return Verdict::Mismatch(Mismatch::Unreadable),
        Row::Absent => None,
        Row::Named(named) => Some(*named),
    };
    if named == Some(cut.row()) {
        return Verdict::Committed;
    }
    let head_is_the_rows = head_group == named.map(|named| named.group);
    if cut.whole {
        return if head_is_the_rows {
            Verdict::Upload(Rule::Whole)
        } else {
            Verdict::Mismatch(Mismatch::HeadMoved)
        };
    }
    match named {
        None => Verdict::Mismatch(Mismatch::NoRow),
        Some(named) if Some(named) != cut.base => Verdict::Mismatch(Mismatch::NotTheBase),
        Some(_) if !head_is_the_rows => Verdict::Mismatch(Mismatch::HeadMoved),
        Some(_) => Verdict::Upload(Rule::Delta),
    }
}

#[cfg(test)]
mod tests {
    use skyzfs_local::inplace::CommittedCut;

    use super::*;
    use crate::row::RowError;

    const STATE: [u8; 16] = *b"the cut state...";
    const OLD: [u8; 16] = *b"an older state..";

    const BASE: CommittedCut = CommittedCut {
        state: STATE,
        number: 4,
        group: 40,
    };

    /// Cut 5 of group 50, onto the base cut 4 of group 40.
    fn delta() -> Uploadable {
        Uploadable {
            state: STATE,
            number: 5,
            group: 50,
            whole: false,
            itself: true,
            base: Some(BASE),
        }
    }

    fn whole() -> Uploadable {
        Uploadable {
            whole: true,
            ..delta()
        }
    }

    fn named(state: [u8; 16], number: u64, group: u64) -> Row {
        Row::Named(CommittedCut {
            state,
            number,
            group,
        })
    }

    /// (a) first: a row that names the cut is its commit, whatever head
    /// says and whether the cut is whole or not.
    #[test]
    fn a_row_naming_the_cut_being_uploaded_says_it_is_committed() {
        for cut in [delta(), whole()] {
            for head in [Some(50), Some(40), None] {
                assert_eq!(
                    judge(&cut, &named(STATE, 5, 50), head),
                    Verdict::Committed,
                    "{cut:?}, head's group {head:?}"
                );
            }
        }
    }

    /// The row names the cut by its state, its number and its group; one
    /// that differs in any of them is not the cut.
    #[test]
    fn a_row_differing_from_the_cut_in_anything_is_not_its_commit() {
        for row in [named(OLD, 5, 50), named(STATE, 6, 50), named(STATE, 5, 51)] {
            assert_ne!(
                judge(&delta(), &row, Some(50)),
                Verdict::Committed,
                "{row:?}"
            );
            assert_ne!(
                judge(&whole(), &row, Some(50)),
                Verdict::Committed,
                "{row:?}"
            );
        }
    }

    #[test]
    fn a_cut_of_part_of_the_volume_is_uploaded_onto_the_base_the_row_names() {
        assert_eq!(
            judge(&delta(), &named(STATE, 4, 40), Some(40)),
            Verdict::Upload(Rule::Delta)
        );
        // The base made by another state: a start that did not take up
        // the cut state counts what the intent map names against it.
        let other = Uploadable {
            base: Some(CommittedCut {
                state: OLD,
                number: 9,
                group: 30,
            }),
            ..delta()
        };
        assert_eq!(
            judge(&other, &named(OLD, 9, 30), Some(30)),
            Verdict::Upload(Rule::Delta)
        );
    }

    #[test]
    fn a_cut_of_part_of_the_volume_against_another_row_is_a_mismatch() {
        let cases = [
            (Row::Absent, None, Mismatch::NoRow),
            (Row::Absent, Some(40), Mismatch::NoRow),
            (named(OLD, 4, 40), Some(40), Mismatch::NotTheBase),
            (named(STATE, 3, 30), Some(30), Mismatch::NotTheBase),
            // Q8: our own later cut that the map does not name.
            (named(STATE, 6, 60), Some(60), Mismatch::NotTheBase),
            (named(STATE, 4, 40), Some(41), Mismatch::HeadMoved),
            (named(STATE, 4, 40), None, Mismatch::HeadMoved),
        ];
        for (row, head, why) in cases {
            assert_eq!(
                judge(&delta(), &row, head),
                Verdict::Mismatch(why),
                "{row:?}, head's group {head:?}"
            );
        }
        let no_base = Uploadable {
            base: None,
            ..delta()
        };
        assert_eq!(
            judge(&no_base, &named(STATE, 4, 40), Some(40)),
            Verdict::Mismatch(Mismatch::NotTheBase)
        );
    }

    /// Q6: a whole cut needs no base. It is uploaded when head's commit is
    /// the row's whatever the row names, and with no row when head's
    /// commit has no group.
    #[test]
    fn a_whole_cut_is_uploaded_onto_the_rows_commit_whatever_the_row_names() {
        for row in [named(STATE, 4, 40), named(OLD, 9, 40), named(STATE, 7, 40)] {
            assert_eq!(
                judge(&whole(), &row, Some(40)),
                Verdict::Upload(Rule::Whole),
                "{row:?}"
            );
        }
        assert_eq!(
            judge(&whole(), &Row::Absent, None),
            Verdict::Upload(Rule::Whole)
        );
        let no_base = Uploadable {
            base: None,
            ..whole()
        };
        assert_eq!(
            judge(&no_base, &Row::Absent, None),
            Verdict::Upload(Rule::Whole)
        );
    }

    #[test]
    fn a_whole_cut_onto_a_head_that_is_not_the_rows_is_a_mismatch() {
        let cases = [
            (named(STATE, 4, 40), Some(41)),
            (named(STATE, 4, 40), None),
            (Row::Absent, Some(1)),
        ];
        for (row, head) in cases {
            assert_eq!(
                judge(&whole(), &row, head),
                Verdict::Mismatch(Mismatch::HeadMoved),
                "{row:?}, head's group {head:?}"
            );
        }
    }

    #[test]
    fn a_row_that_cannot_be_read_is_always_a_mismatch() {
        let row = Row::Unreadable(RowError::Version(2));
        for cut in [delta(), whole()] {
            for head in [Some(40), Some(50), None] {
                assert_eq!(
                    judge(&cut, &row, head),
                    Verdict::Mismatch(Mismatch::Unreadable)
                );
            }
        }
    }
}
