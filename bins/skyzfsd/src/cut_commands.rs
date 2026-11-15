//! The test build's commands about held cuts (M2.2b-2), for the rigs of
//! M2.T3: what the daemon holds, a held cut written out as an upload of
//! it would leave an image, and a cut let go as an upload's commit will.
//!
//! **A stand-in.** M2.5's channel to the uploader replaces this; until
//! then it exists only in the build with test hooks, and nothing but the
//! rigs may build on it. The shape was agreed with the testing session.
//!
//! # The protocol
//!
//! `kill -USR2 <pid>` makes the daemon read the first line of
//! `<volume run dir>/cut-command` after its queue's next turn, run it on
//! the store between turns, and answer in `cut-reply` in the same
//! directory, written whole to `cut-reply.tmp` and renamed, so that a
//! reader never sees half of one. The line begins with a token of the
//! rig's choosing, a word that the reply's last line begins with too, so
//! that the rig knows the reply is for its own command:
//!
//! - `TOKEN held`: one line for each held cut, oldest first,
//!   `number group phase opens blocks`: its number in the count of cuts
//!   (the number the request log's `cut` line carries), the transaction
//!   group of the uberblock that marked it, `uploading`, `waiting` or
//!   `flushing`, `itself` or `rewind` (how it opens, `cuts.rs`), and how
//!   many blocks its map holds. Then `TOKEN ok`.
//! - `TOKEN write N PATH`: writes every block of held cut N's map, as
//!   the cut reads it (from its copy where a write changed it after the
//!   instant), at the block's own offset into the file PATH, which must
//!   be there and absolute. PATH is the rig's copy of the image the
//!   cut is uploaded onto, so after the write it is the volume at the
//!   cut's instant. The map's block numbers go to `PATH.blocks`, one a
//!   line, ascending. The write is done a slice of [`SLICE`] blocks
//!   after each turn of the queue, so the device keeps serving; the
//!   reply comes when it is done: `TOKEN ok K blocks`. Only the cut
//!   being uploaded or the one that waits can be written. Neither file
//!   is synced: a reader on the same machine sees them through the
//!   page cache.
//! - `TOKEN release N`: lets cut N go, as an upload's commit will. Only
//!   the cut being uploaded can be let go. Then `TOKEN ok`.
//!
//! Anything else, or a command the store cannot carry out, is answered
//! `TOKEN refused: REASON` (`- refused: …` when the line has no token).
//!
//! **After the volume has failed** the commands go on, a write under way
//! included. Design §5.2 ("When a cut may leave the machine") has the
//! instance that saw the failure still upload a cut whose flush had
//! completed: each of its blocks is untouched since that flush or was
//! copied before the write that changed it began. A failure fails every
//! flush under way, and each lets go the cut it was making; until it is
//! answered, that cut is refused as any cut whose flush is under way.
//! It is the next start that trusts none of the state.
//!
//! **One command at a time.** A command that comes while a write is
//! under way is refused as busy, and that reply is replaced by the
//! write's when the write ends: the rig waits for each reply before it
//! sends the next command. Signals that come before the daemon has read
//! the file are one. A write that the daemon's stop interrupts gets no
//! reply. A write is refused at its end when a cut was let go or
//! abandoned under it, or when its map grew (an older cut abandoned at
//! the bound gives the cut that waits its map): the rig writes it again.
//!
//! **What it costs.** A write takes the map's block numbers into memory
//! (eight bytes a block; the first cut of a new state is the whole
//! volume, so a 32 GiB volume takes 64 MiB), and `held` counts each
//! cut's map.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write as _};
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};

use skyzfs_blk::{Buf, BLOCK_SIZE};
use skyzfs_local::inplace::CutPhase;

/// The command's file, in the volume's run directory.
pub const COMMAND: &str = "cut-command";
/// The reply's file, in the volume's run directory.
pub const REPLY: &str = "cut-reply";
/// Where the reply is written before it is renamed.
const REPLY_TEMP: &str = "cut-reply.tmp";
/// The blocks a write takes after each turn of the queue: 1 MiB.
pub const SLICE: usize = 256;

/// A held cut, as `held` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// Its number in the count of cuts.
    pub number: u64,
    /// The transaction group of the uberblock that marked it.
    pub group: u64,
    /// Where it stands.
    pub phase: CutPhase,
    /// Whether it opens as itself (rather than by a rewind).
    pub itself: bool,
    /// The blocks in its map.
    pub blocks: u64,
}

/// What the commands need of the store: the in-place store's, on the
/// daemon's files or the tests' simulated disk, and the tests' stand-in.
pub trait Cuts {
    /// Whether the store holds cuts (`serve --hold-cuts`).
    fn holds_cuts(&self) -> bool;
    /// The held cuts, oldest first.
    fn held(&self) -> Vec<Held>;
    /// The blocks in held cut `number`'s map; none if it is not held.
    fn map(&self, number: u64) -> Vec<u64>;
    /// Read block `block` of held cut `number` into `buf`, one block:
    /// whether the block is in the cut's map. `NotFound` if the cut is
    /// not held.
    fn read(&mut self, number: u64, block: u64, buf: &mut [u8]) -> io::Result<bool>;
    /// Let held cut `number`, the one being uploaded, go: whether it was
    /// that cut.
    fn release(&mut self, number: u64) -> bool;
}

impl<D: skyzfs_local::io::Dir> Cuts for skyzfs_local::inplace::InPlace<D> {
    fn holds_cuts(&self) -> bool {
        self.cuts_at_start().is_some()
    }

    fn held(&self) -> Vec<Held> {
        self.held_cuts()
            .into_iter()
            .map(|cut| Held {
                number: cut.number,
                group: cut.state.txg,
                phase: cut.phase,
                itself: cut.itself,
                blocks: self.cut_map(cut.number).len() as u64,
            })
            .collect()
    }

    fn map(&self, number: u64) -> Vec<u64> {
        self.cut_map(number)
    }

    fn read(&mut self, number: u64, block: u64, buf: &mut [u8]) -> io::Result<bool> {
        self.read_cut(number, block, buf)
    }

    fn release(&mut self, number: u64) -> bool {
        self.release_cut(number)
    }
}

/// A command, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Held,
    Write { number: u64, path: PathBuf },
    Release { number: u64 },
}

/// The token and the command of a command line, or why it is none. The
/// token is `-` when the line has none.
fn parse(line: &str) -> (String, Result<Command, String>) {
    let line = line.trim();
    let Some((token, rest)) = line
        .split_once(char::is_whitespace)
        .or((!line.is_empty()).then_some((line, "")))
    else {
        return ("-".into(), Err("an empty command".into()));
    };
    let rest = rest.trim_start();
    let (verb, args) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(verb, args)| (verb, args.trim()));
    let number = |text: &str| {
        text.parse::<u64>()
            .map_err(|_| format!("not a cut's number: {text:?}"))
    };
    let command = match verb {
        "held" if args.is_empty() => Ok(Command::Held),
        "release" if !args.is_empty() && !args.contains(char::is_whitespace) => {
            number(args).map(|number| Command::Release { number })
        }
        "write" => match args.split_once(char::is_whitespace) {
            Some((n, path)) => {
                let path = PathBuf::from(path.trim_start());
                if path.is_absolute() {
                    number(n).map(|number| Command::Write { number, path })
                } else {
                    Err(format!("not an absolute path: {}", path.display()))
                }
            }
            None => Err("write takes a cut's number and a path".into()),
        },
        "held" | "release" => Err(format!("{verb} takes {}", takes(verb))),
        "" => Err("no command after the token".into()),
        _ => Err(format!("not a command: {verb:?}")),
    };
    (token.to_owned(), command)
}

/// What a command takes, for a refusal.
fn takes(verb: &str) -> &'static str {
    match verb {
        "held" => "nothing",
        _ => "a cut's number",
    }
}

/// A phase as `held` says it.
fn phase_said(phase: CutPhase) -> &'static str {
    match phase {
        CutPhase::Uploading => "uploading",
        CutPhase::Waiting => "waiting",
        CutPhase::Flushing => "flushing",
    }
}

/// A write under way.
struct Job {
    token: String,
    number: u64,
    path: PathBuf,
    file: File,
    blocks: BufWriter<File>,
    /// The cut's map, ascending, as it was when the write began.
    map: Vec<u64>,
    /// The next block of `map` to write.
    next: usize,
    /// One block, aligned for the store's direct reads.
    buf: Buf,
}

/// `path` with `.blocks` after it.
fn blocks_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".blocks");
    PathBuf::from(name)
}

/// The commands of one served volume.
pub struct Commands {
    /// The volume's run directory.
    dir: PathBuf,
    job: Option<Job>,
}

impl Commands {
    /// The commands read from and answered in `dir`, the volume's run
    /// directory.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, job: None }
    }

    /// After a turn of the queue: read the command if `asked` (a
    /// `SIGUSR2` came) and carry out what can be carried out now, then
    /// write the next slice of a write under way. Whether a write is
    /// still under way: the caller wakes the queue, so that the next
    /// slice follows at once on an idle device.
    pub fn turn(&mut self, asked: bool, cuts: &mut impl Cuts) -> bool {
        if asked {
            self.read_command(cuts);
        }
        if let Some(job) = &mut self.job {
            if let Some(reply) = slice(job, cuts) {
                self.job = None;
                self.reply(&reply);
            }
        }
        self.job.is_some()
    }

    /// Read and carry out the command in the file.
    fn read_command(&mut self, cuts: &mut impl Cuts) {
        let path = self.dir.join(COMMAND);
        let (token, command) = match std::fs::read_to_string(&path) {
            Ok(text) => parse(text.lines().next().unwrap_or("")),
            Err(e) => ("-".into(), Err(format!("{}: {e}", path.display()))),
        };
        let reply = match command {
            Err(why) => refused(&token, &why),
            Ok(_) if self.job.is_some() => {
                let number = self.job.as_ref().map_or(0, |job| job.number);
                refused(
                    &token,
                    &format!("busy: the write of cut {number} is under way"),
                )
            }
            Ok(_) if !cuts.holds_cuts() => refused(
                &token,
                "held cuts are off: the daemon was started without --hold-cuts",
            ),
            Ok(Command::Held) => held(&token, cuts),
            Ok(Command::Release { number }) => release(&token, number, cuts),
            Ok(Command::Write { number, path }) => match begin(&token, number, path, cuts) {
                Ok(job) => {
                    // Answered when it is done.
                    self.job = Some(job);
                    return;
                }
                Err(reply) => reply,
            },
        };
        self.reply(&reply);
    }

    /// Write `reply` whole, by rename.
    fn reply(&self, reply: &str) {
        let temp = self.dir.join(REPLY_TEMP);
        let written = std::fs::write(&temp, reply)
            .and_then(|()| std::fs::rename(&temp, self.dir.join(REPLY)));
        if let Err(e) = written {
            crate::to_stderr(&format!(
                "skyzfsd: the reply to a command about held cuts could not be written in {}: {e}\n",
                self.dir.display()
            ));
        }
    }
}

/// A refusal's reply.
fn refused(token: &str, why: &str) -> String {
    format!("{token} refused: {why}\n")
}

/// `held`'s reply.
fn held(token: &str, cuts: &impl Cuts) -> String {
    let mut reply = String::new();
    for cut in cuts.held() {
        reply.push_str(&format!(
            "{} {} {} {} {}\n",
            cut.number,
            cut.group,
            phase_said(cut.phase),
            if cut.itself { "itself" } else { "rewind" },
            cut.blocks
        ));
    }
    reply.push_str(&format!("{token} ok\n"));
    reply
}

/// `release`'s reply.
fn release(token: &str, number: u64, cuts: &mut impl Cuts) -> String {
    if cuts.release(number) {
        return format!("{token} ok\n");
    }
    let uploading = cuts
        .held()
        .into_iter()
        .find(|cut| cut.phase == CutPhase::Uploading)
        .map_or("no cut is being uploaded".to_owned(), |cut| {
            format!("the cut being uploaded is {}", cut.number)
        });
    refused(
        token,
        &format!("cut {number} is not the one being uploaded: {uploading}"),
    )
}

/// A write, begun, or the reply that refuses it.
fn begin(token: &str, number: u64, path: PathBuf, cuts: &impl Cuts) -> Result<Job, String> {
    let Some(cut) = cuts.held().into_iter().find(|cut| cut.number == number) else {
        return Err(refused(token, &format!("no cut {number} is held")));
    };
    if cut.phase == CutPhase::Flushing {
        return Err(refused(
            token,
            &format!(
                "cut {number}'s flush has not completed: only the cut being uploaded or the one \
                 that waits can be written"
            ),
        ));
    }
    let file = OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| refused(token, &format!("{}: {e}", path.display())))?;
    let blocks_at = blocks_path(&path);
    let blocks = File::create(&blocks_at)
        .map_err(|e| refused(token, &format!("{}: {e}", blocks_at.display())))?;
    let mut map = cuts.map(number);
    map.sort_unstable();
    Ok(Job {
        token: token.to_owned(),
        number,
        path,
        file,
        blocks: BufWriter::new(blocks),
        map,
        next: 0,
        buf: Buf::zeroed(1),
    })
}

/// Write the next slice of `job`: its reply when it is done, or failed.
fn slice(job: &mut Job, cuts: &mut impl Cuts) -> Option<String> {
    let end = (job.next + SLICE).min(job.map.len());
    for at in job.next..end {
        let block = job.map[at];
        let written = match cuts.read(job.number, block, &mut job.buf) {
            Ok(true) => job.file.write_all_at(&job.buf, block * BLOCK_SIZE),
            // Not in the cut's map after all: the image the cut is
            // uploaded onto holds the block, and so does PATH.
            Ok(false) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Some(refused(
                    &job.token,
                    &format!(
                        "cut {} is no longer held after {at} of {} blocks: it was let go, or \
                         abandoned at the bound or at a copy that could not be made",
                        job.number,
                        job.map.len()
                    ),
                ));
            }
            Err(e) => {
                return Some(refused(
                    &job.token,
                    &format!("read block {block} of cut {}: {e}", job.number),
                ))
            }
        };
        let listed = written
            .map_err(|e| format!("{}: write block {block}: {e}", job.path.display()))
            .and_then(|()| {
                writeln!(job.blocks, "{block}")
                    .map_err(|e| format!("{}: {e}", blocks_path(&job.path).display()))
            });
        if let Err(why) = listed {
            return Some(refused(&job.token, &why));
        }
    }
    job.next = end;
    if job.next < job.map.len() {
        return None;
    }
    if let Err(e) = job.blocks.flush() {
        return Some(refused(
            &job.token,
            &format!("{}: {e}", blocks_path(&job.path).display()),
        ));
    }
    let now = cuts.map(job.number).len();
    if now != job.map.len() {
        return Some(refused(
            &job.token,
            &format!(
                "cut {}'s map grew from {} to {now} blocks during the write (an older cut was \
                 abandoned at the bound and gave it its map): write it again",
                job.number,
                job.map.len()
            ),
        ));
    }
    Some(format!("{} ok {} blocks\n", job.token, job.map.len()))
}

#[cfg(test)]
mod tests;
