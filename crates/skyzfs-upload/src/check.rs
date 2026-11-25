//! The check before each cut (the lead's Q1, Q5 and Q6 of M2.4b, decided
//! 2026-11-17): what head's row and head's commit say of the cut being
//! uploaded.

use skynbd_types::CommitHash;

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
    /// Head's commit is not the row's: not the commit the row names, or
    /// one with a group where there is no row. Something other than this
    /// uploader moved head (an operator's rollback, another writer), and
    /// the uploader does not move it on its own (Q5). Under a row of
    /// version 1, head moved where its group is not the row's.
    HeadMoved,
    /// A cut of part of the volume, and a row of version 1 (written by a
    /// daemon from before F1) that names its base, with head's group the
    /// row's: the delta would have gone up before F1, but the row names no
    /// commit, so head is not known to be the commit the base went up as
    /// (N3 of the review of #134; the lead's decision, 2026-11-24). This
    /// is the first delta after an upgrade. It is refused as any mismatch
    /// is, and the next cut goes up whole and writes version 2. Where
    /// head's group is not the row's, head moved, and the mismatch is
    /// [`Mismatch::HeadMoved`], as before F1.
    RowV1,
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
            Mismatch::RowV1 => "the row names no commit",
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
/// read and head's commit, `head`, and its group, `head_group` (`None`
/// for a commit with no group), in this order:
///
/// 1. the row names the cut (its state, number and group): it is
///    committed, whatever commit the row names;
/// 2. a cut that holds the whole volume is uploaded when head's commit is
///    the row's, whatever cut the row names, or, with no row, when head's
///    commit has no group;
/// 3. a cut that holds part of the volume is uploaded when the row names
///    the cut the intent map names as committed, and head's commit is the
///    row's;
/// 4. else a mismatch, and a row that cannot be read is always one.
///
/// Head's commit is the row's where it is the commit the row names (F1
/// of the review of #122). A row of version 1 names no commit, and the
/// check reads it as the lead decided (2026-11-19):
/// a whole cut, which holds every block and builds on nothing, is
/// uploaded onto it where head's group is the row's, as every row was
/// judged before F1, and its commit writes a row of version 2; a cut of
/// part of the volume is never put onto it, for its delta would rest on
/// a history the row does not name ([`Mismatch::RowV1`] where head's
/// group is the row's, [`Mismatch::HeadMoved`] where it is not), and the
/// held cuts are abandoned so that the next goes up whole.
pub fn judge(cut: &Uploadable, row: &Row, head: CommitHash, head_group: Option<u64>) -> Verdict {
    let named = match row {
        Row::Unreadable(_) => return Verdict::Mismatch(Mismatch::Unreadable),
        Row::Absent => None,
        Row::Named(named) => Some(*named),
    };
    if named.map(|named| named.cut) == Some(cut.row()) {
        return Verdict::Committed;
    }
    // Whether head's commit is the row's: by the commit the row names, or,
    // for a row of version 1, by its group alone.
    let head_is_the_rows = match named {
        None => head_group.is_none(),
        Some(named) => match named.commit {
            Some(commit) => commit == head,
            None => head_group == Some(named.cut.group),
        },
    };
    if cut.whole {
        return if head_is_the_rows {
            Verdict::Upload(Rule::Whole)
        } else {
            Verdict::Mismatch(Mismatch::HeadMoved)
        };
    }
    match named {
        None => Verdict::Mismatch(Mismatch::NoRow),
        Some(named) if Some(named.cut) != cut.base => Verdict::Mismatch(Mismatch::NotTheBase),
        // A row of version 1 whose group is head's: only the commit it
        // does not name stands between the cut and a delta (N3).
        Some(named) if named.commit.is_none() && head_is_the_rows => {
            Verdict::Mismatch(Mismatch::RowV1)
        }
        Some(named) if named.commit != Some(head) => Verdict::Mismatch(Mismatch::HeadMoved),
        Some(_) => Verdict::Upload(Rule::Delta),
    }
}

#[cfg(test)]
mod tests {
    use skyzfs_local::inplace::CommittedCut;

    use super::*;
    use crate::row::{Named, RowError};

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
            rewound: 0,
        }
    }

    fn whole() -> Uploadable {
        Uploadable {
            whole: true,
            ..delta()
        }
    }

    /// The commit of the cut of group `group` in these tests.
    fn commit_of(group: u64) -> CommitHash {
        [group as u8; 32]
    }

    /// Head at the commit of the cut of group `group`: the row's, where
    /// the row names that cut.
    fn head(group: u64) -> (CommitHash, Option<u64>) {
        (commit_of(group), Some(group))
    }

    /// Head at another commit with group `group`: another writer's, or a
    /// rollback to a snapshot of another cut with that group (n1 and n2
    /// of the review of #122).
    fn other(group: u64) -> (CommitHash, Option<u64>) {
        ([group as u8 ^ 0xff; 32], Some(group))
    }

    /// Head at a commit with no group.
    const NO_GROUP: (CommitHash, Option<u64>) = ([0xee; 32], None);

    /// A row of version 2 that names the cut and its commit.
    fn named(state: [u8; 16], number: u64, group: u64) -> Row {
        Row::Named(Named {
            cut: CommittedCut {
                state,
                number,
                group,
            },
            commit: Some(commit_of(group)),
        })
    }

    /// A row of version 1, which names no commit.
    fn named_v1(state: [u8; 16], number: u64, group: u64) -> Row {
        Row::Named(Named {
            cut: CommittedCut {
                state,
                number,
                group,
            },
            commit: None,
        })
    }

    fn judged(cut: &Uploadable, row: &Row, (commit, group): (CommitHash, Option<u64>)) -> Verdict {
        judge(cut, row, commit, group)
    }

    /// (a) first: a row that names the cut is its commit, whatever head
    /// says, whether the cut is whole or not, and whatever commit the row
    /// names.
    #[test]
    fn a_row_naming_the_cut_being_uploaded_says_it_is_committed() {
        for cut in [delta(), whole()] {
            for at in [head(50), head(40), other(50), NO_GROUP] {
                for row in [named(STATE, 5, 50), named_v1(STATE, 5, 50)] {
                    assert_eq!(
                        judged(&cut, &row, at),
                        Verdict::Committed,
                        "{cut:?}, {row:?}, head {at:?}"
                    );
                }
            }
        }
    }

    /// The row names the cut by its state, its number and its group; one
    /// that differs in any of them is not the cut.
    #[test]
    fn a_row_differing_from_the_cut_in_anything_is_not_its_commit() {
        for row in [named(OLD, 5, 50), named(STATE, 6, 50), named(STATE, 5, 51)] {
            assert_ne!(
                judged(&delta(), &row, head(50)),
                Verdict::Committed,
                "{row:?}"
            );
            assert_ne!(
                judged(&whole(), &row, head(50)),
                Verdict::Committed,
                "{row:?}"
            );
        }
    }

    #[test]
    fn a_cut_of_part_of_the_volume_is_uploaded_onto_the_base_the_row_names() {
        assert_eq!(
            judged(&delta(), &named(STATE, 4, 40), head(40)),
            Verdict::Upload(Rule::Delta)
        );
        // The base made by another state: a start that did not take up
        // the cut state counts what the intent map names against it.
        let other_base = Uploadable {
            base: Some(CommittedCut {
                state: OLD,
                number: 9,
                group: 30,
            }),
            ..delta()
        };
        assert_eq!(
            judged(&other_base, &named(OLD, 9, 30), head(30)),
            Verdict::Upload(Rule::Delta)
        );
    }

    #[test]
    fn a_cut_of_part_of_the_volume_against_another_row_is_a_mismatch() {
        let cases = [
            (Row::Absent, NO_GROUP, Mismatch::NoRow),
            (Row::Absent, head(40), Mismatch::NoRow),
            (named(OLD, 4, 40), head(40), Mismatch::NotTheBase),
            (named(STATE, 3, 30), head(30), Mismatch::NotTheBase),
            // Q8: our own later cut that the map does not name.
            (named(STATE, 6, 60), head(60), Mismatch::NotTheBase),
            (named(STATE, 4, 40), head(41), Mismatch::HeadMoved),
            (named(STATE, 4, 40), NO_GROUP, Mismatch::HeadMoved),
            // n2 of the review of #122: head at another commit of the
            // row's group.
            (named(STATE, 4, 40), other(40), Mismatch::HeadMoved),
            // A row of version 1 names no commit: no delta onto it. Where
            // head's group is the row's, that is all that stands in the
            // way (N3 of the review of #134); where it is not, head moved.
            (named_v1(STATE, 4, 40), head(40), Mismatch::RowV1),
            (named_v1(STATE, 4, 40), other(40), Mismatch::RowV1),
            (named_v1(STATE, 4, 40), head(41), Mismatch::HeadMoved),
            (named_v1(STATE, 4, 40), NO_GROUP, Mismatch::HeadMoved),
            (named_v1(OLD, 4, 40), head(40), Mismatch::NotTheBase),
        ];
        for (row, at, why) in cases {
            assert_eq!(
                judged(&delta(), &row, at),
                Verdict::Mismatch(why),
                "{row:?}, head {at:?}"
            );
        }
        let no_base = Uploadable {
            base: None,
            ..delta()
        };
        assert_eq!(
            judged(&no_base, &named(STATE, 4, 40), head(40)),
            Verdict::Mismatch(Mismatch::NotTheBase)
        );
    }

    /// Q6: a whole cut needs no base. It is uploaded when head's commit is
    /// the row's whatever the row names, and with no row when head's
    /// commit has no group. Under a row of version 1, head's commit is
    /// the row's where its group is.
    #[test]
    fn a_whole_cut_is_uploaded_onto_the_rows_commit_whatever_the_row_names() {
        for row in [named(STATE, 4, 40), named(OLD, 9, 40), named(STATE, 7, 40)] {
            assert_eq!(
                judged(&whole(), &row, head(40)),
                Verdict::Upload(Rule::Whole),
                "{row:?}"
            );
        }
        for at in [head(40), other(40)] {
            assert_eq!(
                judged(&whole(), &named_v1(STATE, 4, 40), at),
                Verdict::Upload(Rule::Whole),
                "a row of version 1, head {at:?}"
            );
        }
        assert_eq!(
            judged(&whole(), &Row::Absent, NO_GROUP),
            Verdict::Upload(Rule::Whole)
        );
        let no_base = Uploadable {
            base: None,
            ..whole()
        };
        assert_eq!(
            judged(&no_base, &Row::Absent, NO_GROUP),
            Verdict::Upload(Rule::Whole)
        );
    }

    #[test]
    fn a_whole_cut_onto_a_head_that_is_not_the_rows_is_a_mismatch() {
        let cases = [
            (named(STATE, 4, 40), head(41)),
            (named(STATE, 4, 40), NO_GROUP),
            // n2's case under a whole cut.
            (named(STATE, 4, 40), other(40)),
            (Row::Absent, head(1)),
            (named_v1(STATE, 4, 40), head(41)),
            (named_v1(STATE, 4, 40), NO_GROUP),
        ];
        for (row, at) in cases {
            assert_eq!(
                judged(&whole(), &row, at),
                Verdict::Mismatch(Mismatch::HeadMoved),
                "{row:?}, head {at:?}"
            );
        }
    }

    #[test]
    fn a_row_that_cannot_be_read_is_always_a_mismatch() {
        let row = Row::Unreadable(RowError::Version(3));
        for cut in [delta(), whole()] {
            for at in [head(40), head(50), other(40), NO_GROUP] {
                assert_eq!(
                    judged(&cut, &row, at),
                    Verdict::Mismatch(Mismatch::Unreadable)
                );
            }
        }
    }
}
