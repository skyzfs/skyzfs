//! The upload driver: a state machine over the cut being uploaded, the
//! store ([`Local`]) and object storage ([`Remote`]), stepped by its
//! caller.
//!
//! A cut goes through these stages, a step each, with as many steps of
//! reading as it has slices:
//!
//! 1. **Check**: head's row and head, against the cut ([`judge`]). The
//!    row names the cut: let it go. A mismatch: abandon every held cut.
//!    Else the upload begins.
//! 2. **Read**: a slice of the cut's blocks at its instant, written into
//!    the engine (an all-zero block as a zero extent), and the segments
//!    sealed so far PUT.
//! 3. **Sync**, where a block was read from a copy: the copies file is
//!    synced before the commit, and a sync that fails abandons the cut.
//! 4. **Commit**: head and the row swapped together, the row's swap
//!    expecting the row as the check read it; then the cut let go.
//!
//! A step that fails on object storage leaves the stage as it was, so
//! that the step made again goes on: a read's writes stay in the engine,
//! and a commit is made again only once the row and head have been read
//! again. A row that then names the cut says that the commit had landed;
//! a row or head that moved gives the cut up for a new check. A cut that
//! goes before its commit (abandoned at the bound, or its read or its
//! sync failed) drops its writes, and the cut that took its map is
//! checked and uploaded next. A cut that goes after its commit has
//! landed and before it is let go is named in the intent map as the
//! committed cut instead, so that the cut that took its map goes up as a
//! delta onto it (or whole, where the cut named was whole: its heir took
//! a whole cut's map).
//!
//! A cut committed that the store cannot let go stops the driver, for
//! good: the intent map's pages could not be written, the volume has
//! failed, and the cut stays held as the one being uploaded. The store
//! would offer it again and the check find it committed again, and only
//! the cut being uploaded can be let go, so no later cut can leave from
//! this instance (F2 of the review of M2.4b-1). A failure of the volume
//! that leaves the release alone stops nothing: design §5.2 has the
//! instance that saw it still upload a cut whose flush had completed.
//! Whether that instance stays up for it is M2.5's.
//!
//! A death at any point leaves the cut held, with head and the row as
//! they were or as the commit made them: the next instance's check finds
//! which.

use skyzfs_blk::{Buf, BLOCK_SIZE, BLOCK_USIZE};

use crate::check::{judge, Mismatch, Rule, Verdict};
use crate::local::{Local, Released, Uploadable};
use crate::remote::{Remote, RemoteError};
use crate::row::{self, Row};

/// The blocks a slice reads, one turn of the queue's thread each: 256 KiB.
/// Provisional, for M2.4c and M2.T4 to measure: the larger, the fewer
/// round trips to the queue's thread and the longer each turn it spends
/// reading.
pub const SLICE_BLOCKS: usize = 64;

/// What a step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// No cut is being uploaded: none is held whose flush has completed.
    Idle,
    /// The row names the cut being uploaded: its commit had landed. It is
    /// let go, and nothing is uploaded.
    FoundCommitted {
        /// The cut.
        number: u64,
    },
    /// The check found what the cut is made against: its upload begins.
    Began {
        /// The cut.
        number: u64,
        /// How it is uploaded.
        rule: Rule,
    },
    /// A slice of the cut was read and written into the engine.
    Read {
        /// The cut.
        number: u64,
        /// The blocks read.
        blocks: u64,
        /// The blocks written as zeros with nothing read: holes of the
        /// image when the cut state began that nothing wrote up to the
        /// cut's instant (M2.4e).
        zeroes: u64,
    },
    /// The copies the cut was read from are durable.
    CopiesSynced {
        /// The cut.
        number: u64,
    },
    /// The cut is committed in object storage and let go.
    Uploaded {
        /// The cut.
        number: u64,
        /// How it was uploaded.
        rule: Rule,
        /// Its blocks.
        blocks: u64,
    },
    /// The cut was committed, but it had gone from the store (abandoned
    /// at the bound) before it could be let go. The driver names it in
    /// the intent map as the committed cut ([`Local::name_committed`]).
    UploadedNotHeld {
        /// The cut.
        number: u64,
        /// Whether the map names it now, so that the next check finds the
        /// row to be the base and the next cut goes up as a delta onto it
        /// (whole where that cut holds the whole volume). Where the store
        /// refused, or where a death or a lost header undoes the naming,
        /// the next check finds a row the map does not name: a mismatch,
        /// and one whole upload. Where the map's pages could not be
        /// written, the volume has failed, but the map in memory names
        /// the cut all the same: this instance uploads the next cut onto
        /// it, and stops at that cut's release ([`Step::Kept`]) if the
        /// map still cannot be written.
        named: bool,
    },
    /// The cut is committed, by this driver or found so, but the store
    /// kept it ([`Released::Kept`]): the intent map's pages could not be
    /// written, and the volume has failed. The driver stops, for good
    /// ([`Doing::Stopped`]): every step after this one is
    /// [`Step::Idle`].
    Kept {
        /// The cut.
        number: u64,
    },
    /// The row or head did not hold what the held cuts were made
    /// against: every held cut is abandoned, and the next cut holds the
    /// whole volume.
    Mismatch {
        /// Why.
        why: Mismatch,
        /// The cuts abandoned, oldest first.
        abandoned: Vec<u64>,
    },
    /// The cut went before its commit: its writes are dropped, and the
    /// cut that took its map is uploaded next.
    Lost {
        /// The cut.
        number: u64,
        /// Why.
        why: String,
    },
    /// A commit that failed found the row or head moved when they were
    /// read again: its writes are dropped, and the cut is checked anew.
    GaveUp {
        /// The cut.
        number: u64,
    },
}

/// What the driver's next step does, for the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    /// Checks the cut being uploaded, when there is one.
    Checking,
    /// Reads a slice of the cut under way.
    Reading,
    /// Syncs the copies the cut was read from.
    Syncing,
    /// Commits the cut, or reads the row and head again before it makes
    /// a commit that failed again.
    Committing,
    /// Nothing, for good: the store kept a cut committed ([`Step::Kept`]).
    Stopped,
}

/// What the last check found, for the status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Checked {
    /// No check yet.
    #[default]
    Unchecked,
    /// The row and head held what the cut was made against, or named the
    /// cut itself.
    Matched,
    /// They did not.
    Mismatched(Mismatch),
}

/// The driver's figures since it began, for the status (M2.4c).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Figures {
    /// The cuts committed by this driver.
    pub uploaded: u64,
    /// Among them, those that held the whole volume.
    pub uploaded_whole: u64,
    /// Their blocks, read and written.
    pub blocks: u64,
    /// Their blocks written as zeros with nothing read: holes of the image
    /// when the cut state began that nothing wrote up to the cut's instant
    /// (M2.4e). Not in `blocks`.
    pub zero_blocks: u64,
    /// The cuts the check found committed already, let go with nothing
    /// uploaded.
    pub found_committed: u64,
    /// The cuts committed (by this driver or found so) that had gone from
    /// the store before they could be let go.
    pub not_held: u64,
    /// Among them, those the intent map names now as the committed cut.
    pub named: u64,
    /// The cuts committed that the store kept ([`Step::Kept`]): one at
    /// most, since the driver stops at it.
    pub kept: u64,
    /// The mismatches.
    pub mismatches: u64,
    /// The cuts they abandoned.
    pub abandoned_at_mismatch: u64,
    /// The cuts that went before their commit.
    pub lost: u64,
    /// The commits given up because the row or head moved.
    pub gave_up: u64,
    /// The steps that failed on object storage.
    pub errors: u64,
    /// The last such failure.
    pub last_error: Option<RemoteError>,
    /// What the last check found.
    pub checked: Checked,
    /// How the last cut uploaded was.
    pub last_rule: Option<Rule>,
}

/// A cut under way, from its check to its commit.
#[derive(Debug, Clone)]
struct Upload {
    cut: Uploadable,
    rule: Rule,
    /// The row as the check read it: what the commit's swap of the row
    /// expects.
    expected: Option<Vec<u8>>,
    /// The next block of the cut's map to read.
    from: u64,
    /// Whether a block was read from a copy.
    copied: bool,
    /// The blocks read so far.
    blocks: u64,
    /// The blocks written as zeros so far, with nothing read.
    zeroes: u64,
}

/// Where the driver stands.
#[derive(Debug, Clone)]
enum Stage {
    Check,
    Read(Upload),
    Sync(Upload),
    /// `failed`: the commit was tried and failed, with a conflict or not.
    Commit {
        upload: Upload,
        failed: Option<bool>,
    },
    /// The store kept a cut committed: nothing more is done.
    Stopped,
}

/// The upload driver ([module docs](self)).
pub struct Driver<L, R> {
    local: L,
    remote: R,
    stage: Stage,
    buf: Buf,
    figures: Figures,
}

impl<L: Local, R: Remote> Driver<L, R> {
    /// A driver that reads slices of [`SLICE_BLOCKS`].
    pub fn new(local: L, remote: R) -> Self {
        Self::with_slices(local, remote, SLICE_BLOCKS)
    }

    /// A driver that reads slices of `blocks` blocks, at least one.
    pub fn with_slices(local: L, remote: R, blocks: usize) -> Self {
        Self {
            local,
            remote,
            stage: Stage::Check,
            buf: Buf::zeroed(blocks.max(1)),
            figures: Figures::default(),
        }
    }

    /// The store.
    pub fn local(&mut self) -> &mut L {
        &mut self.local
    }

    /// Object storage.
    pub fn remote(&mut self) -> &mut R {
        &mut self.remote
    }

    /// The figures since the driver began.
    pub fn figures(&self) -> &Figures {
        &self.figures
    }

    /// The cut under way, from its check to its commit.
    pub fn under_way(&self) -> Option<u64> {
        match &self.stage {
            Stage::Check | Stage::Stopped => None,
            Stage::Read(upload) | Stage::Sync(upload) | Stage::Commit { upload, .. } => {
                Some(upload.cut.number)
            }
        }
    }

    /// What the next step does.
    pub fn doing(&self) -> Doing {
        match self.stage {
            Stage::Check => Doing::Checking,
            Stage::Read(_) => Doing::Reading,
            Stage::Sync(_) => Doing::Syncing,
            Stage::Commit { .. } => Doing::Committing,
            Stage::Stopped => Doing::Stopped,
        }
    }

    /// Take the next step. An error from object storage leaves the stage
    /// as it was, for the step to be taken again.
    pub async fn step(&mut self) -> Result<Step, RemoteError> {
        let stage = std::mem::replace(&mut self.stage, Stage::Check);
        let (stage, result) = match stage {
            Stage::Check => self.check().await,
            Stage::Read(upload) => self.read(upload).await,
            Stage::Sync(upload) => self.sync(upload).await,
            Stage::Commit { upload, failed } => self.commit(upload, failed).await,
            Stage::Stopped => (Stage::Stopped, Ok(Step::Idle)),
        };
        self.stage = stage;
        if let Err(e) = &result {
            self.figures.errors += 1;
            self.figures.last_error = Some(e.clone());
        }
        result
    }

    /// Step until there is no cut to upload: the tests', and a caller's
    /// that has nothing else to do. Stops at the first error.
    pub async fn run(&mut self) -> Result<Vec<Step>, RemoteError> {
        let mut steps = Vec::new();
        loop {
            let step = self.step().await?;
            if step == Step::Idle {
                return Ok(steps);
            }
            steps.push(step);
        }
    }

    async fn check(&mut self) -> (Stage, Result<Step, RemoteError>) {
        let Some(cut) = self.local.uploadable().await else {
            return (Stage::Check, Ok(Step::Idle));
        };
        let seen = match self.remote.observe().await {
            Ok(seen) => seen,
            Err(e) => return (Stage::Check, Err(e)),
        };
        let row = Row::of(seen.row.as_deref());
        match judge(&cut, &row, seen.head_group) {
            Verdict::Committed => {
                self.figures.checked = Checked::Matched;
                let step = self.found_committed(&cut).await;
                (Self::after(&step), Ok(step))
            }
            Verdict::Mismatch(why) => {
                let abandoned = self.local.abandon_all().await;
                self.figures.checked = Checked::Mismatched(why);
                self.figures.mismatches += 1;
                self.figures.abandoned_at_mismatch += abandoned.len() as u64;
                (Stage::Check, Ok(Step::Mismatch { why, abandoned }))
            }
            Verdict::Upload(rule) => {
                self.figures.checked = Checked::Matched;
                let upload = Upload {
                    cut,
                    rule,
                    expected: seen.row,
                    from: 0,
                    copied: false,
                    blocks: 0,
                    zeroes: 0,
                };
                let began = Step::Began {
                    number: cut.number,
                    rule,
                };
                (Stage::Read(upload), Ok(began))
            }
        }
    }

    /// The row names `cut`: let it go.
    async fn found_committed(&mut self, cut: &Uploadable) -> Step {
        let number = cut.number;
        match self.local.release(number).await {
            Released::LetGo => {
                self.figures.found_committed += 1;
                Step::FoundCommitted { number }
            }
            Released::Kept => self.kept(number),
            Released::Gone => self.not_held(cut).await,
        }
    }

    /// The store kept cut `number`, committed: stop.
    fn kept(&mut self, number: u64) -> Step {
        self.figures.kept += 1;
        Step::Kept { number }
    }

    /// The stage after a step that ends a cut: the check of the next, or
    /// nothing more after [`Step::Kept`].
    fn after(step: &Step) -> Stage {
        match step {
            Step::Kept { .. } => Stage::Stopped,
            _ => Stage::Check,
        }
    }

    /// `cut` is committed, but it had gone from the store before it could
    /// be let go: name it as the committed cut.
    async fn not_held(&mut self, cut: &Uploadable) -> Step {
        let named = self.local.name_committed(cut.number, cut.group).await;
        self.figures.not_held += 1;
        if named {
            self.figures.named += 1;
        }
        Step::UploadedNotHeld {
            number: cut.number,
            named,
        }
    }

    async fn read(&mut self, mut upload: Upload) -> (Stage, Result<Step, RemoteError>) {
        // What an earlier step left sealed goes first, so that a store
        // that cannot be reached does not let the cut pile up in memory.
        if let Err(e) = self.remote.put_sealed().await {
            return (Stage::Read(upload), Err(e));
        }
        let number = upload.cut.number;
        let buf = std::mem::replace(&mut self.buf, Buf::zeroed(0));
        let (buf, read) = self.local.read(number, upload.from, buf).await;
        self.buf = buf;
        let slice = match read {
            Ok(slice) => slice,
            Err(e) => return (Stage::Check, Ok(self.lost(number, e.to_string()))),
        };
        if let Err(e) = write_slice(&mut self.remote, &slice.blocks, &slice.zeroes, &self.buf) {
            // Not reached for a cut of this volume: the engine refuses only
            // a write past its end. Its writes are dropped with the cut.
            self.remote.discard();
            return (Stage::Check, Err(e));
        }
        let blocks = slice.blocks.len() as u64;
        let zeroes = slice.zeroes.iter().map(|&(_, count)| count).sum::<u64>();
        upload.blocks += blocks;
        upload.zeroes += zeroes;
        upload.copied |= slice.copied;
        let next = match slice.next {
            Some(from) => {
                upload.from = from;
                Stage::Read(upload)
            }
            None if upload.copied => Stage::Sync(upload),
            None => Stage::Commit {
                upload,
                failed: None,
            },
        };
        // The slice is in the engine whatever its PUTs do: a failure here
        // goes on from the next slice, after the PUTs made again.
        let put = self.remote.put_sealed().await;
        (
            next,
            put.map(|()| Step::Read {
                number,
                blocks,
                zeroes,
            }),
        )
    }

    async fn sync(&mut self, upload: Upload) -> (Stage, Result<Step, RemoteError>) {
        let number = upload.cut.number;
        match self.local.sync_copies(number).await {
            Ok(()) => (
                Stage::Commit {
                    upload,
                    failed: None,
                },
                Ok(Step::CopiesSynced { number }),
            ),
            Err(e) => (Stage::Check, Ok(self.lost(number, e.to_string()))),
        }
    }

    async fn commit(
        &mut self,
        upload: Upload,
        failed: Option<bool>,
    ) -> (Stage, Result<Step, RemoteError>) {
        let number = upload.cut.number;
        if let Some(conflict) = failed {
            // The commit failed, and no error says whether it landed:
            // read the row and head again before it is made again.
            let seen = match self.remote.observe().await {
                Ok(seen) => seen,
                Err(e) => return (Stage::Commit { upload, failed }, Err(e)),
            };
            if Row::of(seen.row.as_deref()) == Row::Named(upload.cut.row()) {
                self.remote.reset();
                let step = self.uploaded(&upload).await;
                return (Self::after(&step), Ok(step));
            }
            if conflict || seen.row != upload.expected || !seen.current {
                self.remote.reset();
                self.figures.gave_up += 1;
                return (Stage::Check, Ok(Step::GaveUp { number }));
            }
        }
        // A cut that went since its check is not committed: the cut that
        // took its map is uploaded against the same base instead.
        let held = self.local.uploadable().await;
        if held.map(|cut| cut.row()) != Some(upload.cut.row()) {
            return (
                Stage::Check,
                Ok(self.lost(number, "it went before its commit".into())),
            );
        }
        let row = row::encode(&upload.cut.row());
        match self
            .remote
            .commit(upload.cut.group, upload.expected.clone(), row)
            .await
        {
            Ok(()) => {
                let step = self.uploaded(&upload).await;
                (Self::after(&step), Ok(step))
            }
            Err(e) => {
                let conflict = e == RemoteError::Conflict;
                (
                    Stage::Commit {
                        upload,
                        failed: Some(conflict),
                    },
                    Err(e),
                )
            }
        }
    }

    /// `upload`'s cut is committed: let it go.
    async fn uploaded(&mut self, upload: &Upload) -> Step {
        let number = upload.cut.number;
        self.figures.uploaded += 1;
        self.figures.blocks += upload.blocks;
        self.figures.zero_blocks += upload.zeroes;
        self.figures.last_rule = Some(upload.rule);
        if upload.rule == Rule::Whole {
            self.figures.uploaded_whole += 1;
        }
        match self.local.release(number).await {
            Released::LetGo => Step::Uploaded {
                number,
                rule: upload.rule,
                blocks: upload.blocks,
            },
            Released::Kept => self.kept(number),
            Released::Gone => self.not_held(&upload.cut).await,
        }
    }

    /// Cut `number` went before its commit: drop its writes.
    fn lost(&mut self, number: u64, why: String) -> Step {
        self.remote.discard();
        self.figures.lost += 1;
        Step::Lost { number, why }
    }
}

/// Write a slice into the engine: its blocks, a block of `buf` each in
/// order, each run of adjacent blocks in one write and each run of all-zero
/// blocks as one zero extent; then its runs of zeros (`(first, count)`),
/// each as one zero extent. The two never share a block (the store reads
/// no block it names as zeros), so their order does not matter.
fn write_slice<R: Remote>(
    remote: &mut R,
    blocks: &[u64],
    zeroes: &[(u64, u64)],
    buf: &[u8],
) -> Result<(), RemoteError> {
    let data = |i: usize| &buf[i * BLOCK_USIZE..(i + 1) * BLOCK_USIZE];
    let zero = |i: usize| data(i).iter().all(|&byte| byte == 0);
    let mut i = 0;
    while i < blocks.len() {
        let mut end = i + 1;
        while end < blocks.len() && blocks[end] == blocks[end - 1] + 1 && zero(end) == zero(i) {
            end += 1;
        }
        let offset = blocks[i] * BLOCK_SIZE;
        if zero(i) {
            remote.write_zeroes(offset, (end - i) as u64 * BLOCK_SIZE)?;
        } else {
            remote.write(offset, &buf[i * BLOCK_USIZE..end * BLOCK_USIZE])?;
        }
        i = end;
    }
    for &(first, count) in zeroes {
        remote.write_zeroes(first * BLOCK_SIZE, count * BLOCK_SIZE)?;
    }
    Ok(())
}
