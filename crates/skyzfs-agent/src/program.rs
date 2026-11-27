//! The program `skyzfs-agent` (M2.6c-3): its command line, what it prints,
//! and its exit codes. The binary (`bins/skyzfs-agent`) builds the
//! adapters ([`crate::zfs::cli`], [`crate::daemon`], [`crate::record`])
//! and calls [`execute`]; everything it decides is here, so that it is
//! tested on the engine's fakes.
//!
//! # Commands
//!
//! ```text
//! skyzfs-agent publish --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      [--limit-s S] [--expiry-s E]
//! skyzfs-agent settle  --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      [--limit-s S]
//! skyzfs-agent recover --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      --name T (--give-up | --retry) [--limit-s S]
//! skyzfs-agent run     --volume VOL --run RUN --device DEV --pool POOL --dir DIR
//!                      [--every-s S] [--expiry-s E] [--keep N]
//!                      [--abandoned-keep-s S]
//! skyzfs-agent record  --dir DIR
//! ```
//!
//! - `VOL` is the volume's directory, as `skyzfsd create` made it: the
//!   agent reads the volume's id from its file `remote` ([`volume_id`]).
//! - `RUN` is the run directory the daemon serves with (`skyzfsd serve
//!   --run-dir`), shared by every daemon on the node: the volume's
//!   channel is the socket `RUN/v-<dev>-<ino>/control`, named by the
//!   device and inode numbers of `VOL` in hex, as the daemon names it
//!   ([`skyzfs_control::socket_path`]). A node passes the same run
//!   directory to the daemon and to the agent.
//! - `DEV` is the volume's block device, as `skyzfsd serve` prints it
//!   (`ready /dev/ublkbN`).
//! - `DIR` is the agent's own directory of the volume: its record and
//!   its manifests ([`crate::record`]). One agent at a time holds it.
//!
//! `publish` begins a publish, then settles every publish under way
//! ([`crate::engine::Agent::settle`]) until `--limit-s` seconds have
//! passed ([`LIMIT_DEFAULT`]), and in any case through one pass over
//! the publishes under way, however long the reads before it take.
//! `--expiry-s` is how long the daemon may keep the request pending
//! (the protocol's default where it is not given, at most
//! [`skyzfs_control::EXPIRY_MAX`]). `settle` settles every publish
//! under way and begins none; it is how a rig, or a person, finishes
//! what a stopped agent left. `recover` records a person's choice for one
//! publish under way, then settles: `--give-up` gives it up as
//! [`crate::record::Why::Person`] (its request withdrawn first, a
//! publish already made kept), and `--retry` clears a stop for a person
//! ([`crate::record::Person::Stopped`]) so that it is planned again.
//! `record` prints the record ([`dump`]) without its lock, and changes
//! nothing. Each of these ends; `run` (below) runs until it is told to
//! end.
//!
//! # `run`
//!
//! `run` holds the record's directory and publishes at each tick, every
//! `--every-s` seconds ([`EVERY_DEFAULT`]) on the clock that does not
//! step, the first at once ([`schedule`]). Each tick's round syncs the
//! pool past the groups of a history that a rewind import abandoned, if
//! there are any (below), begins a publish, and then settles every
//! publish under way until the next tick:
//! a publish that does not settle within its round is settled by the
//! rounds after it, as `settle` would, and up to
//! [`crate::record::UNDER_WAY_MAX`] are under way at once (a begin
//! refused for that is reported and the round still settles). It takes
//! no `--limit-s`: its limit is the tick.
//!
//! After each round, one line on standard output:
//!
//! ```text
//! round <n> ok|failed took_ms=<ms> late_ms=<ms> skipped=<k> begun=<t|-> under_way=<count>
//!   synced=<k> bound=0|1 [early_ms=<ms>] [reopen=held]
//! ```
//!
//! on one line.
//!
//! - `took_ms`: how long the round ran; `late_ms`: how long after its
//!   tick it began. `early_ms`, only on a round begun before its tick
//!   (news of a lost volume, or a quick retry of the step), says how
//!   long before; such a round does not use up its tick, whose round
//!   still comes on it (D1 of the review of #173). `reopen=held`, only
//!   on a round whose reopen the cap held back (below).
//! - `skipped`: ticks that passed while this round ran and had no round
//!   of their own. Rounds never overlap: the next round begins as this
//!   one ends, for the last tick that passed, and its `late_ms` says by
//!   how much.
//! - `begun`: the publish this round began; `-` where it began none:
//!   its begin failed; the round's syncs failed, or left the daemon's
//!   newest cut still behind, or an end was asked during them (below);
//!   or the engine's reading of the record failed (a failed round,
//!   below).
//! - `under_way`: publishes still under way as the round ended.
//! - `synced`: the syncs the round ran, and that returned, to pass the
//!   groups of an abandoned history (below); 0 in a round that found
//!   none.
//! - `bound`: 1 where the round found the syncs at their bound with the
//!   daemon's newest cut still behind (below), else 0. The round whose
//!   last sync met the bound reads 0 where that sync made no cut (it
//!   stopped for that), and the rounds after it read 1. Such a round
//!   begins no publish, and nor does any round after it until a cut
//!   opens as itself or the daemon holds no cut; each is `ok` unless
//!   another call of it failed (its settle). A monitor that watches
//!   `run` watches this field as well as `failed` (the lead's ruling 3
//!   on #170).
//! - `ok`: no engine call of the round returned an error. It does not
//!   say that anything moved: a round that waited on the daemon
//!   throughout is `ok`.
//! - `failed`: an engine call of the round failed, the two refusals of
//!   a begin included (8 under way, the pool's premise); each error is
//!   on standard error as `skyzfs-agent: round <n>: <error>`, and the
//!   next tick tries again. A sync after a rewind import that fails
//!   fails its round too (below). After an error that may leave the
//!   process unlike its record (any but those two refusals), the engine
//!   reads the record again, as a new process would: at once after an
//!   error of the syncs or of a begin, before the round settles; at the
//!   next round after a settle's error. A reading that fails is tried again at the next
//!   round, which begins and settles nothing until it succeeds: its
//!   `begun=-` then says that no begin ran, and its `under_way` is the
//!   count the process had before. So `begun=-` can also follow a
//!   `snap/` line for the very publish whose begin failed: its intent
//!   was saved before the save failed, and the round settled it.
//!
//!   The rule is wide on purpose (the lead's D1 on #167), and it has a
//!   cost while the daemon is down: each round that fails reads the
//!   record again, walks the pool's holds again at its sweep, and tells
//!   again every `stopped` line and every report of a stall that the
//!   restart made it forget.
//!
//! **Retention** (M2.7b-2): after its begin and before it settles, a
//! round retires the names published past the newest `--keep`
//! ([`crate::engine::KEEP_DEFAULT`], at most
//! [`crate::record::KEEP_MAX`]) and lets the names given up go after
//! `--abandoned-keep-s` ([`crate::engine::ABANDONED_KEEP_DEFAULT`]), at
//! most [`crate::engine::STEPS_MAX`] steps in a round
//! ([`Agent::retain`]). Each name that leaves says so on standard
//! output, before it leaves the record (so a kill between the two says
//! it again at the next run):
//!
//! ```text
//! retired <t>
//! forgotten <t> (<why>)
//! stopped <t> retire refused other: <the daemon's text>
//! ```
//!
//! `retired`: `snap/<t>` retired where it named the publish's commit,
//! and each member's `@t` of the GUID recorded destroyed. `forgotten`:
//! a name given up that leaves with its local snapshot, for a person to
//! destroy. `stopped`: `snap/<t>` names another commit; nothing is
//! retired or destroyed, and the name stays. What retention could not
//! do at a round is a report on standard error (`not-retired`,
//! `no-guids`), told once and again when its reason changes, and does
//! not fail the round; an error of saving the record does.
//!
//! **After a rewind import** (`zpool import -T`) no cut opens as itself
//! until the pool's groups have passed those of the history the import
//! abandoned, so no publish can be made (design §5.2). Each round first
//! reads the daemon's `cuts`, and while its newest cut does not open as
//! itself runs the agent's sync task, one at a time, `cuts` read again
//! after each, until one does ([`Agent::sync_past`]); the pool's premise
//! is checked before the first. The syncs stop with the newest cut still
//! behind where the daemon counted no cut after one, where an end was
//! asked for, or after [`crate::engine::SYNC_PAST_MAX`] syncs in this
//! process with no cut that opens as itself (more than a rewind import
//! can leave). Such a round begins no publish (`begun=-`), says why on
//! standard error, and is still `ok`:
//!
//! ```text
//! skyzfs-agent: round <n>: no publish begun: the newest cut, of group <g>,
//!   does not open as itself: <why>
//! ```
//!
//! on one line.
//!
//! It still settles the publishes under way. Once the bound is met, the
//! rounds of this process sync no more until they see a cut that opens
//! as itself (a change of the pool that writes groups past the abandoned
//! ones makes one; a bare `zpool sync` of an idle pool writes none) or
//! the daemon holding no cut; a restart of the engine does not start the
//! count again.
//! Only syncs that returned count toward it: a sync that fails fails its
//! round, as a begin's error does (the engine reads the record again at
//! once, and the round begins nothing), and is not counted; the round
//! line counts the syncs before it that returned. An end asked during
//! the syncs begins no publish, even where the last of them made the
//! newest cut open as itself: the round line says `begun=-` and the
//! process ends.
//!
//! What the bound does not cover (the lead's rulings on #170):
//! - It is the process's, and not saved: a supervisor that starts `run`
//!   again after it ends syncs up to [`crate::engine::SYNC_PAST_MAX`]
//!   times at each start while the newest cut stays behind.
//! - The tick does not limit a round's syncs: a round runs them to
//!   their end, and may run past its tick by up to that many syncs.
//!   Rounds still never overlap (`skipped`, `late_ms`).
//! - A sync that fails is not counted (it may have written nothing):
//!   where every sync fails, only the tick limits them, one a round, each
//!   round `failed` with the error.
//!
//! **A volume that failed in this boot** (M2.7c; [`crate::engine::Failed`]).
//! Each round first reads the volume's failure note, which a daemon
//! leaves beside its socket when its volume fails or a start refuses it.
//! - **No note of this boot**: the round goes on.
//! - **The note says no client held the device**: the round says so on
//!   standard error, touches no pool (no sync, begin or settle: Codex's P2
//!   on #173), is not failed, and `run` goes on at its ticks. A start with
//!   `--acknowledge-failure` serves the volume afresh and removes the
//!   note; the next round then reads none, and `run` ends with 12
//!   (below).
//! - **The note says a client held it, or may have, or the note is torn**
//!   (a file there that is not a whole note, which the daemon's start
//!   counts as held too): the volume is lost for this boot, and so is a
//!   pool on it. A pool that only writes never suspends by itself, so
//!   the round makes the agent's pool read ([`Agent::lose`]): `zpool
//!   reopen` within 10 s, then a look at the pool's state every 250 ms
//!   for at most 10 s, until it reads SUSPENDED. **The look, not the
//!   reopen's status, is the signal**: the reopen returns with status 0
//!   before the pool suspends, and is refused where zed suspended it
//!   first; its error is on standard error and changes nothing. No
//!   reopen is made of a pool that is not imported or reads SUSPENDED
//!   already. The round begins and settles nothing, prints
//!
//!   ```text
//!   lost <pool> volume=<hex> note=whole|torn reason=<word|-> held=yes|unknown
//!     step=not-imported|suspended|reopened|held|failed reopen=ok|failed|- state=<STATE|-|unread>
//!     looked_ms=<ms|-> verdict=lost|failed|retry
//!   ```
//!
//!   on one line ([`lost_line`]), then its round line. A torn note reads
//!   `note=torn reason=- held=yes`. `state` is the pool's state at the
//!   last look, `-` where it is not imported, and `unread` where a look
//!   after the reopen failed (its error on standard error; the reopen's
//!   outcome is still said). What follows is the `verdict` (the lead's
//!   rulings on F2 and F4 of the review of #173):
//!   - `lost`: the pool reads SUSPENDED. `run` ends with 11
//!     ([`exit::LOST`]). A service manager must not start it again on
//!     11: the volume is not served again before a reboot, and a `run`
//!     started again on the same note finds the pool suspended, makes no
//!     second reopen, and ends with 11 again.
//!   - `failed`: the pool is not imported, before the reopen or after
//!     it, whatever the note says it held: nothing is suspended, and
//!     nothing can be. The round says the volume failed on standard
//!     error, as for a note that no client held, is not failed, and
//!     `run` goes on; a later start of the daemon may serve the volume.
//!   - `retry`: the step showed neither. The pool's state could not be
//!     read (`step=failed`, or `state=unread`), or the pool did not read
//!     SUSPENDED at the last look (the reopen refused or not), or the cap
//!     held the reopen back (`step=held`). The round is failed, and the
//!     step is taken again: [`QUICK_RETRY`] after the round ends, for
//!     [`QUICK_RETRIES`] rounds in a row, then at the ticks (the lead's
//!     ruling on D1 of the review of #173). A round with another verdict
//!     ends the run of quick retries.
//!
//!   **The cap on reopens** (the lead's ruling on the review of #173).
//!   Once [`REOPEN_CAP`] reopens of one note in this process have left
//!   the pool unsuspended (a wrong `--pool`, a layout that keeps it
//!   going), the pool is reopened only at every [`REOPEN_CAP`]th round
//!   after, and standard error says so once. The rounds in between read
//!   its state only (`step=held reopen=- state=<STATE> looked_ms=-`),
//!   so a pool that zed suspends meanwhile is still found lost; they
//!   take no quick retry. A reopen restarts a scrub under way, which is
//!   what the cap spares. Another note starts the count again.
//!
//!   A round that reads such a note touches nothing but the step, whatever
//!   its verdict: no sync, no begin and no settle of what is under way,
//!   which stays in the record. That is this program's reading (the
//!   rulings say what the verdicts end in, not what else the round does):
//!   a pool whose volume is gone is given no more ZFS calls than the step
//!   needs.
//!
//! - **A note that cannot be read** (an error other than its absence,
//!   or a boot id that cannot be read): the round says the error, is
//!   failed, and touches nothing; the next round reads it again. The note
//!   may be a lost volume's, so no publish is begun beside it, as
//!   `publish` refuses with 1.
//!
//! **The note is watched between rounds** (F1 of the review of #173; the
//! lead's ruling). A pool that only writes goes on acknowledging
//! synchronous writes into a lost device, so the exposure grows with each
//! second until the step. While a round's engine works, and while `run`
//! waits for the next tick, the note is read about every second
//! ([`crate::engine::NOTE_EVERY`]; [`Agent::watch_note`]); a read is of a
//! small file and makes no I/O on the pool. A note that makes the volume
//! lost, and is not the one the round read at its start, stops the engine
//! where it asks whether to go on (before a sync after a rewind import,
//! before a begin after syncs, before each pass of the settle) and ends
//! the wait: the round's line follows, and the next round begins at once,
//! early for its tick (`early_ms`), and takes the step. Rounds stay
//! `--every-s` apart otherwise, but for the quick retries above. A step of the engine under way is not cut, so the note is
//! seen after at most about a second plus that step.
//!
//! `publish` and `settle` read the note too, before anything else, and
//! touch no pool: a lost volume is refused with 11, one that failed with
//! no client holding its device with 1 (its daemon is gone), with words
//! on standard error. They do not look at the pool, so they refuse a
//! lost note whether or not its pool is imported. `recover` and `record`
//! do not read it.
//!
//! **A new device, or a new attachment** (the lead's D4 on #167). `run`
//! ends with 12 ([`exit::MOVED`]) after the round line of the round in
//! which it saw the daemon's attachment change (a new attach of the
//! volume's device, at each `cuts`; a take-over keeps it), or the
//! volume's device number change (at each check of the pool's premise),
//! from what it first saw in this process, with the change on standard
//! error. A service manager starts it again, with `--device` as the
//! device now is: the engine reads both afresh at its start.
//!
//! The codes 11 and 12 are the agent's. `skyzfsd` has codes 9 and 10 of
//! its own, with other meanings for a service manager, so the agent uses
//! none of them (the lead's ruling on F5 of the review of #173).
//!
//! `publish` does not sync before its begin: a publish asked while the
//! pool is below the abandoned groups pends, and the planner's own syncs
//! carry it (the case M2's criterion tests).
//!
//! The outcome lines below are printed as each publish leaves the
//! record or stops, among the round lines. A `snap/` line is printed
//! before the record forgets its publish, so that a run killed in
//! between prints it again when it starts: at least once. An
//! `abandoned` line is printed the same way (M2.7b-2): the give-up is
//! saved first as being given up (phase `abandoning`), then told, then
//! moved to the names given up, so that a run killed between the save
//! and the move tells it again: at least once, and twice where the kill
//! fell after the line. A `retired` line is printed before the record
//! lets the name go, with the same rule.
//!
//! `run` ends with 0 at SIGTERM or SIGINT, between two rounds of the
//! engine or while it waits for a tick, within [`WAKE`]; what is under
//! way stays in the record for the next start. A signal that comes
//! while the engine works ends `run` once its step is done (a sync after
//! a rewind import, a begin, or a pass over the publishes under way),
//! before the count and the sweep that would open the next, and before
//! the next sync or a begin after syncs: a ZFS call is not cut, and the
//! step's other calls run first, each within its own time limit. Two
//! windows count as part of the step that follows them: a signal during
//! the round's first reading of `cuts`, where it finds no cut behind,
//! lets the begin run; one during the premise's reading before the
//! first sync lets that sync run. A second signal ends
//! it at once with 1. It ends with 6 after the round line of a round that
//! met a record that is corrupt or another volume's, or holds it would
//! release with no agent's marker: a person judges those. It ends with
//! 1 or 8 where it cannot start (the record unread, or the directory
//! held), and with 6 where the record it starts on is one a person
//! judges. It ends with 11 after the round that showed a lost volume's
//! pool suspended, and with 12 after
//! the round that saw its device or the daemon's attachment change
//! (above). `recover` of a volume whose `run` holds its directory exits 8:
//! a person stops `run`, recovers, and starts it again.
//!
//! # What it prints
//!
//! On standard output, one line for each publish that left the record
//! or stopped in this run, in the order they did ([`outcome_line`]):
//!
//! ```text
//! snap/<t> <commit, 64 hex digits>      published
//! abandoned <t> <why>                   given up (the words of [`why_word`])
//! stopped <t> <why, in words>           stopped for a person; still under way
//! unsettled <t>                         still under way at the time limit
//! retired <t>                           retired by `run`'s retention
//! forgotten <t> (<why>)                 let go by `run`'s retention, its
//!                                       local snapshot left for a person
//! ```
//!
//! On standard error, each report as it is made, before the agent acts
//! on it ([`report_line`]), and each error.
//!
//! # Exit codes
//!
//! `publish` and `recover` exit by the outcome of their own publish;
//! `settle` by the worst of all ([`worst`]): see [`exit`]. An error of
//! the engine exits by [`exit_of_error`], whatever was settled before
//! it: a `publish` whose own publish was made, and whose run then
//! failed on another, prints its `snap/` line and exits 1. 5 comes only
//! from `publish`, where the pool's premise refuses the publish it
//! begins; a publish under way whose premise is refused later is given
//! up (`abandoned <t> premise`, 3), under any command. 11 and 12 are
//! `run`'s ends above; `publish` and `settle` also refuse a lost volume
//! with 11 ([`exit`]).
//!
//! `record` of a directory with no record prints `absent` and exits 0;
//! of a path that is not a directory it exits 1.
//!
//! # Test hooks
//!
//! Options for the rigs, which a build without the feature `test-hooks`
//! of the binary refuses as a usage error ([`Hooks`]):
//!
//! - `--kill-at EFFECT:before|after[:N]`: at the `N`th reach (from 1;
//!   1 where it is not given) of that point of the engine
//!   ([`crate::engine::Point`]; the effects' words are
//!   [`crate::engine::Effect::word`]), the line of [`kill_line`] on
//!   standard error, then `SIGKILL` to the agent itself: no unwinding,
//!   nothing flushed after it.
//! - `--trace-points FILE`: each point reached appended to `FILE` as a
//!   line ([`trace_line`]), written before the effect and before a kill
//!   at the same point.
//! - `--assist-after-s S`: the time before a sync helps a pending
//!   request ([`crate::plan::ASSIST_AFTER`] otherwise).
//! - `--age-max-s S`: the bound on a publish's age
//!   ([`crate::plan::AGE_MAX`] otherwise).
//!
//! # What is this module's reading
//!
//! The design names no command line, exit codes or output; these are
//! this module's, made for the rigs' needs (testing's list for M2.6c,
//! approved by the lead). `--assist-after-s` and `--age-max-s` are test
//! hooks, not options of a production build: the plan fixes both values.
//! The volume's id is read from `remote`'s line `id` and not checked
//! against the volume's name: the daemon checks that at each `serve`.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use skyzfs_control::{hex, EXPIRY_MAX};

use crate::engine::{
    Agent, Choice, Clock, Daemon, Effect, Error, Failed, Note, Outcome, Past, Point, Points, Step,
    Stuck, Verdict, QUICK_RETRIES, QUICK_RETRY, REOPEN_CAP,
};
use crate::plan::Report;
use crate::record::{Keep, Left, Loaded, Person, Phase, Retire, Why};
use crate::zfs::Zfs;

/// How long `publish`, `settle` and `recover` settle where `--limit-s`
/// is not given: three hours, past a publish's age bound
/// ([`crate::plan::AGE_MAX`], two hours) plus the longest expiry and its
/// margin, so that any publish whose daemon answers settles within it.
pub const LIMIT_DEFAULT: Duration = Duration::from_secs(3 * 60 * 60);

/// The time between two rounds of `run` where `--every-s` is not given:
/// the product's snapshot a minute (design §9, M2's exit criteria).
pub const EVERY_DEFAULT: Duration = Duration::from_secs(60);

/// How long `run` sleeps at most between two looks at whether it is
/// told to end, while it waits for the next tick.
pub const WAKE: Duration = Duration::from_millis(250);

/// The program's exit codes.
pub mod exit {
    /// Published (`publish`, `recover`), or every publish published or
    /// none under way (`settle`).
    pub const PUBLISHED: i32 = 0;
    /// Any other error: ZFS, the daemon at `begin`, the file system, the
    /// record not saved, [`crate::record::UNDER_WAY_MAX`] publishes
    /// under way, a name `recover` does not find under way.
    pub const ERROR: i32 = 1;
    /// The command line is not one the program takes.
    pub const USAGE: i32 = 2;
    /// Given up.
    pub const ABANDONED: i32 = 3;
    /// Stopped for a person: still under way, its hold kept.
    pub const STOPPED: i32 = 4;
    /// The pool is refused a publish (its premise): nothing was taken.
    pub const PREMISE: i32 = 5;
    /// The record is corrupt, or another volume's, or the sweep would
    /// release holds from a directory with no agent's marker: nothing was
    /// done, and a person judges.
    pub const RECORD: i32 = 6;
    /// Still under way when the time limit passed.
    pub const UNSETTLED: i32 = 7;
    /// Another agent holds the record's directory.
    pub const HELD: i32 = 8;
    /// The volume is lost for this boot: its daemon failed, or refused
    /// it, while a client held its device, and `run` shows the pool
    /// SUSPENDED (`lost` line, `verdict=lost`). Not to be started again
    /// before a reboot: a restart on the same note ends with 11 again,
    /// with no second reopen. `publish` and `settle` refuse with it
    /// where the note is there. 11 and not 9: `skyzfsd`'s exit codes 9
    /// and 10 are another program's, with other meanings (the lead's
    /// ruling on F5 of the review of #173).
    pub const LOST: i32 = 11;
    /// `run` saw the daemon's attachment or the volume's device change:
    /// to be started again, on the device as it is now (the lead's D4
    /// on #167). 12 and not 10, as for [`LOST`].
    pub const MOVED: i32 = 12;
}

/// What the program is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Begin a publish, then settle.
    Publish(Run),
    /// Settle.
    Settle(Run),
    /// Record a person's choice for the publish `name`, then settle.
    Recover {
        /// The volume, the daemon and the pool.
        run: Run,
        /// The publish's name.
        name: String,
        /// The choice.
        choice: Choice,
    },
    /// Begin a publish every `every`, and settle, until told to end
    /// ([`schedule`]).
    Run {
        /// The volume, the daemon and the pool.
        run: Run,
        /// The time between two rounds' ticks.
        every: Duration,
        /// What retention keeps.
        kept: Kept,
    },
    /// Print the record in `dir`.
    Record {
        /// The agent's directory of the volume.
        dir: PathBuf,
    },
    /// Print the usage.
    Help,
}

/// What `publish`, `settle` and `recover` run on ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The volume's directory.
    pub volume: PathBuf,
    /// The daemon's directory of its run.
    pub run: PathBuf,
    /// The volume's block device.
    pub device: PathBuf,
    /// The pool.
    pub pool: String,
    /// The agent's directory of the volume.
    pub dir: PathBuf,
    /// How long to settle.
    pub limit: Duration,
    /// The request's expiry (`publish`); `None` for the protocol's
    /// default.
    pub expiry: Option<Duration>,
    /// The test hooks.
    pub hooks: Hooks,
}

/// What `run`'s retention keeps ([`crate::engine::Agent::retain`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    /// `--keep`: the names published kept, from 1 to
    /// [`crate::record::KEEP_MAX`].
    pub keep: usize,
    /// `--abandoned-keep-s`: how long a name given up is kept.
    pub abandoned_keep: Duration,
}

impl Default for Kept {
    fn default() -> Self {
        Kept {
            keep: crate::engine::KEEP_DEFAULT,
            abandoned_keep: crate::engine::ABANDONED_KEEP_DEFAULT,
        }
    }
}

/// The test hooks ([module docs](self), "Test hooks").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    /// `--kill-at`.
    pub kill_at: Option<KillAt>,
    /// `--trace-points`.
    pub trace_points: Option<PathBuf>,
    /// `--assist-after-s`.
    pub assist_after: Option<Duration>,
    /// `--age-max-s`.
    pub age_max: Option<Duration>,
}

/// `--kill-at EFFECT:before|after[:N]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillAt {
    /// The point.
    pub point: Point,
    /// Its reach, from 1, at which the agent dies.
    pub nth: u64,
}

/// A command line that is not one the program takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage(pub String);

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The usage, printed with `--help` and after a usage error.
pub const USAGE: &str = "\
usage:
  skyzfs-agent publish --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       [--limit-s S] [--expiry-s E]
  skyzfs-agent settle  --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       [--limit-s S]
  skyzfs-agent recover --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       --name T (--give-up | --retry) [--limit-s S]
  skyzfs-agent run     --volume VOL --run RUN --device DEV --pool POOL --dir DIR
                       [--every-s S] [--expiry-s E] [--keep N]
                       [--abandoned-keep-s S]
  skyzfs-agent record  --dir DIR
where:
  VOL  the volume's directory, as skyzfsd create made it
  RUN  the run directory of skyzfsd serve (its --run-dir, /run/skyzfsd by
       default); the channel is RUN/v-<dev>-<ino>/control, <dev> and <ino>
       those of VOL in hex
  DEV  the volume's block device, as skyzfsd serve prints it
  DIR  the agent's own directory of the volume
test hooks (a build with the feature test-hooks):
  --kill-at EFFECT:before|after[:N]  --trace-points FILE
  --assist-after-s S  --age-max-s S
exit: 0 published, 1 error, 2 usage, 3 abandoned, 4 stopped for a person,
      5 premise refused (publish), 6 record corrupt or another volume's,
      or no agent's marker where holds would be released,
      7 unsettled at the limit, 8 another agent holds the record;
      run: 0 told to end (SIGTERM, SIGINT), 6 as above, 1 another error
      at its start, 8 as above, 11 the volume lost and its pool shown
      suspended (not to be started again before a reboot), 12 the device
      or the daemon's attachment changed (to be started again);
      publish, settle: 11 the volume lost (the agent's 11 and 12 are not
      skyzfsd's codes)";

/// Parse the arguments after the program's name. `hooks` says whether
/// the build takes the test hooks.
pub fn parse(args: &[String], hooks: bool) -> Result<Command, Usage> {
    let usage = |why: String| Err(Usage(why));
    let Some((verb, rest)) = args.split_first() else {
        return usage("no command".into());
    };
    if matches!(verb.as_str(), "--help" | "-h" | "help") {
        return Ok(Command::Help);
    }
    let mut options: Vec<(String, Option<String>)> = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if matches!(word.as_str(), "--help" | "-h") {
            return Ok(Command::Help);
        }
        let Some(key) = word.strip_prefix("--") else {
            return usage(format!("{word:?} is not an option"));
        };
        let (key, value) = match key.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None if FLAGS.contains(&key) => (key.to_string(), None),
            None => match words.next() {
                // An option's value is not the next option: `--name
                // --give-up` is a name forgotten, not a name (S3 of
                // #149's review). A value that begins with `--` is given
                // as `--key=--value`. Where a value is looked for,
                // `--help` asks for help, but `-h` is taken as the value,
                // as any word with one dash is (nit 3 of the next look at
                // #149).
                Some(value) if value == "--help" => return Ok(Command::Help),
                Some(value) if value.starts_with("--") => {
                    return usage(format!(
                        "--{key} needs a value, not the option {value}; \
                         give such a value as --{key}={value}"
                    ))
                }
                Some(value) => (key.to_string(), Some(value.clone())),
                None => return usage(format!("--{key} needs a value")),
            },
        };
        if options.iter().any(|(k, _)| *k == key) {
            return usage(format!("--{key} is given twice"));
        }
        options.push((key, value));
    }
    let mut take = Options { options, hooks };
    let command = match verb.as_str() {
        "publish" => {
            let mut run = take.run()?;
            run.expiry = take.expiry()?;
            Command::Publish(run)
        }
        "settle" => Command::Settle(take.run()?),
        "run" => {
            // A round settles until the next tick, not for `--limit-s`.
            if take.options.iter().any(|(k, _)| k == "limit-s") {
                return usage("run takes no option --limit-s".into());
            }
            let mut run = take.run()?;
            run.expiry = take.expiry()?;
            let every = take.seconds("every-s")?.unwrap_or(EVERY_DEFAULT);
            let kept = take.kept()?;
            Command::Run { run, every, kept }
        }
        "recover" => {
            let run = take.run()?;
            let name = take.required("name")?;
            let choice = match (take.flag("give-up")?, take.flag("retry")?) {
                (true, false) => Choice::GiveUp,
                (false, true) => Choice::Retry,
                _ => return usage("recover takes one of --give-up and --retry".into()),
            };
            Command::Recover { run, name, choice }
        }
        "record" => Command::Record {
            dir: take.required("dir")?.into(),
        },
        other => return usage(format!("no command {other:?}")),
    };
    match take.options.first() {
        Some((key, _)) => usage(format!("{verb} takes no option --{key}")),
        None => Ok(command),
    }
}

/// The options that take no value.
const FLAGS: [&str; 2] = ["give-up", "retry"];

/// The options not yet taken.
struct Options {
    options: Vec<(String, Option<String>)>,
    hooks: bool,
}

impl Options {
    fn take(&mut self, key: &str) -> Option<Option<String>> {
        let at = self.options.iter().position(|(k, _)| k == key)?;
        Some(self.options.remove(at).1)
    }

    fn optional(&mut self, key: &str) -> Result<Option<String>, Usage> {
        match self.take(key) {
            None => Ok(None),
            Some(Some(value)) if !value.is_empty() => Ok(Some(value)),
            Some(_) => Err(Usage(format!("--{key} needs a value"))),
        }
    }

    fn required(&mut self, key: &str) -> Result<String, Usage> {
        self.optional(key)?
            .ok_or_else(|| Usage(format!("--{key} is needed")))
    }

    fn flag(&mut self, key: &str) -> Result<bool, Usage> {
        match self.take(key) {
            None => Ok(false),
            Some(None) => Ok(true),
            Some(Some(_)) => Err(Usage(format!("--{key} takes no value"))),
        }
    }

    /// A whole number of seconds, at least 1.
    fn seconds(&mut self, key: &str) -> Result<Option<Duration>, Usage> {
        let Some(value) = self.optional(key)? else {
            return Ok(None);
        };
        match value.parse::<u64>() {
            Ok(s) if s >= 1 => Ok(Some(Duration::from_secs(s))),
            _ => Err(Usage(format!(
                "--{key}: {value:?} is not a whole number of seconds, at least 1"
            ))),
        }
    }

    /// Refuse the test hook `key` in a build without them.
    fn hook_allowed(&self, key: &str) -> Result<(), Usage> {
        if !self.hooks && self.options.iter().any(|(k, _)| k == key) {
            return Err(Usage(format!(
                "--{key} is a test hook: this build has no feature test-hooks"
            )));
        }
        Ok(())
    }

    fn hook(&mut self, key: &str) -> Result<Option<String>, Usage> {
        self.hook_allowed(key)?;
        self.optional(key)
    }

    fn hook_seconds(&mut self, key: &str) -> Result<Option<Duration>, Usage> {
        self.hook_allowed(key)?;
        self.seconds(key)
    }

    /// `--keep` and `--abandoned-keep-s`.
    fn kept(&mut self) -> Result<Kept, Usage> {
        let mut kept = Kept::default();
        if let Some(value) = self.optional("keep")? {
            let max = crate::record::KEEP_MAX;
            match value.parse::<usize>() {
                Ok(n) if (1..=max).contains(&n) => kept.keep = n,
                _ => {
                    return Err(Usage(format!(
                        "--keep: {value:?} is not a whole number from 1 to {max}"
                    )))
                }
            }
        }
        if let Some(keep) = self.seconds("abandoned-keep-s")? {
            kept.abandoned_keep = keep;
        }
        Ok(kept)
    }

    /// `--expiry-s`, at most [`EXPIRY_MAX`].
    fn expiry(&mut self) -> Result<Option<Duration>, Usage> {
        let expiry = self.seconds("expiry-s")?;
        match expiry {
            Some(expiry) if expiry > EXPIRY_MAX => Err(Usage(format!(
                "--expiry-s: at most {} s",
                EXPIRY_MAX.as_secs()
            ))),
            _ => Ok(expiry),
        }
    }

    fn run(&mut self) -> Result<Run, Usage> {
        let hooks = Hooks {
            kill_at: self.hook("kill-at")?.as_deref().map(kill_at).transpose()?,
            trace_points: self.hook("trace-points")?.map(PathBuf::from),
            assist_after: self.hook_seconds("assist-after-s")?,
            age_max: self.hook_seconds("age-max-s")?,
        };
        Ok(Run {
            volume: self.required("volume")?.into(),
            run: self.required("run")?.into(),
            device: self.required("device")?.into(),
            pool: self.required("pool")?,
            dir: self.required("dir")?.into(),
            limit: self.seconds("limit-s")?.unwrap_or(LIMIT_DEFAULT),
            expiry: None,
            hooks,
        })
    }
}

/// `EFFECT:before|after[:N]`.
pub fn kill_at(text: &str) -> Result<KillAt, Usage> {
    let bad = |why: &str| Usage(format!("--kill-at {text:?}: {why}"));
    let mut parts = text.split(':');
    let effect = parts
        .next()
        .and_then(Effect::from_word)
        .ok_or_else(|| bad("no such effect"))?;
    let after = match parts.next() {
        Some("before") => false,
        Some("after") => true,
        _ => return Err(bad("before or after")),
    };
    let nth = match parts.next() {
        None => 1,
        Some(n) => match n.parse::<u64>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(bad("N is a whole number, at least 1")),
        },
    };
    if parts.next().is_some() {
        return Err(bad("too many parts"));
    }
    Ok(KillAt {
        point: Point { effect, after },
        nth,
    })
}

/// "before" or "after".
fn side(after: bool) -> &'static str {
    if after {
        "after"
    } else {
        "before"
    }
}

/// The line of standard error before a kill (testing's item 1).
pub fn kill_line(at: &KillAt) -> String {
    format!(
        "skyzfs-agent: test hook --kill-at: killed at {} {} ({})\n",
        at.point.effect.word(),
        side(at.point.after),
        at.nth
    )
}

/// The line `--trace-points` appends for a point reached at `ns` on
/// `CLOCK_MONOTONIC`, for the publish `name` (empty where it is for none;
/// [`crate::engine::Points`]): four fields between tabs.
pub fn trace_line(ns: u64, point: Point, name: &str) -> String {
    format!(
        "{ns}\t{}\t{}\t{name}\n",
        point.effect.word(),
        side(point.after)
    )
}

/// The engine's points for `hooks` ([module docs](self)): each point
/// written to `trace`, then `die` called with [`kill_line`] at the
/// chosen reach. `now_ns` reads `CLOCK_MONOTONIC`. A trace that cannot
/// be written stops the engine there: a rig would read a trace with a
/// point missing as a point not reached. `None` where there is no hook.
pub fn points(
    kill_at: Option<KillAt>,
    mut trace: Option<Box<dyn Write>>,
    mut now_ns: impl FnMut() -> u64 + 'static,
    mut die: impl FnMut(&str) + 'static,
    mut err: Box<dyn Write>,
) -> Option<Points> {
    if kill_at.is_none() && trace.is_none() {
        return None;
    }
    let mut reached = 0u64;
    Some(Box::new(move |point: Point, name: &str| {
        if let Some(out) = &mut trace {
            let line = trace_line(now_ns(), point, name);
            if let Err(e) = out.write_all(line.as_bytes()).and_then(|()| out.flush()) {
                let _ = writeln!(err, "skyzfs-agent: --trace-points: {e}; stopped");
                return true;
            }
        }
        if let Some(at) = kill_at {
            if at.point == point {
                reached += 1;
                if reached == at.nth {
                    die(&kill_line(&at));
                }
            }
        }
        false
    }))
}

/// The word of `why`, as `abandoned` lines and the record's dump give it.
pub fn why_word(why: Why) -> &'static str {
    match why {
        Why::ManifestLost => "manifest-lost",
        Why::ManifestTooLarge => "manifest-too-large",
        Why::Destroyed => "destroyed",
        Why::Replaced => "replaced",
        Why::Premise => "premise",
        Why::NameTaken => "name-taken",
        Why::Rewound => "rewound",
        Why::NoProgress => "no-progress",
        Why::TooOld => "too-old",
        Why::Stopped => "stopped",
        Why::Person => "person",
        Why::NotWhole => "not-whole",
    }
}

/// The line of standard output for a publish that left the record or
/// stopped ([module docs](self)).
pub fn outcome_line(name: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Published(commit) => format!("snap/{name} {}\n", hex(commit)),
        Outcome::Abandoned(why) => format!("abandoned {name} {}\n", why_word(*why)),
        Outcome::Stopped(why) => format!("stopped {name} {why}\n"),
        Outcome::Retired => format!("retired {name}\n"),
        Outcome::Forgotten(why) => format!("forgotten {name} ({why})\n"),
    }
}

/// The line of standard error for a report: its first word after the
/// name says which, for a rig to match.
pub fn report_line(name: &str, report: &Report) -> String {
    let what = match report {
        Report::SnapshotGone => "snapshot-gone: snap/t is this publish's, and pool@t or a \
             member's @t is gone; only opening its cut tells whether it holds pool@t"
            .to_string(),
        Report::NoProgress(n) => {
            format!("no-progress {n}: asked {n} times in a row with no cut counted")
        }
        Report::Refused(code) => {
            format!("refused {}: given up on the daemon's refusal", code.word())
        }
        Report::HeldAway => "held-away: older than its bound, its hold kept while the daemon \
             does not answer"
            .to_string(),
        Report::NotWhole(names) => not_whole(names),
        Report::Unreadable(why) => format!(
            "unreadable: the pool's datasets could not be read, read again at the next round: {}",
            one_line(why)
        ),
        Report::NotRetired(why) => format!(
            "not-retired: tried again at the next round: {}",
            one_line(why)
        ),
        Report::NoGuids => "no-guids: given up with no member GUIDs recorded; its @t is not \
             destroyed, and the name is forgotten after --abandoned-keep-s"
            .to_string(),
    };
    format!("skyzfs-agent: report {name} {what}\n")
}

/// `text` on one line: each line break, and the space about it, one
/// `; `.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The datasets named in a `not-whole` report at most (N7 of #153's
/// review: a report must not carry a pool's every dataset).
pub const NOT_WHOLE_NAMED: usize = 20;

/// The text of a `not-whole` report: the datasets that broke the last
/// take, at most [`NOT_WHOLE_NAMED`] of them.
///
/// The names are separated by `, ` and the count of those left out
/// follows a `; `: a dataset's name may hold a space but neither a comma
/// nor a semicolon (`zfs_namecheck`), so each name reads as one (N13 of
/// #153's review).
fn not_whole(names: &[String]) -> String {
    let given_up = "given up after its takes in a row were not one whole take of the pool \
         or not as read before them";
    if names.is_empty() {
        return format!("not-whole: {given_up}; the datasets are not known");
    }
    let shown = names
        .iter()
        .take(NOT_WHOLE_NAMED)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let more = names.len().saturating_sub(NOT_WHOLE_NAMED);
    if more == 0 {
        format!("not-whole: {given_up}; datasets: {shown}")
    } else {
        format!("not-whole: {given_up}; datasets: {shown}; and {more} more")
    }
}

/// The exit code of one outcome; `None` is a publish still under way.
pub fn exit_of(outcome: Option<&Outcome>) -> i32 {
    match outcome {
        Some(Outcome::Published(_)) => exit::PUBLISHED,
        Some(Outcome::Abandoned(_)) => exit::ABANDONED,
        Some(Outcome::Stopped(_)) => exit::STOPPED,
        // Retention's, made by `run` only, which exits by none of these:
        // a name that left the record and asks nothing of the caller.
        Some(Outcome::Retired | Outcome::Forgotten(_)) => exit::PUBLISHED,
        None => exit::UNSETTLED,
    }
}

/// The exit code of `settle`: [`exit::UNSETTLED`] where a publish is
/// still under way at the limit, else [`exit::STOPPED`] where one
/// stopped, else [`exit::ABANDONED`] where one was given up, else
/// [`exit::PUBLISHED`].
pub fn worst(outcomes: &[(String, Outcome)], unsettled: bool) -> i32 {
    let any = |code: i32| outcomes.iter().any(|(_, o)| exit_of(Some(o)) == code);
    if unsettled {
        exit::UNSETTLED
    } else if any(exit::STOPPED) {
        exit::STOPPED
    } else if any(exit::ABANDONED) {
        exit::ABANDONED
    } else {
        exit::PUBLISHED
    }
}

/// The exit code of an error of the engine.
pub fn exit_of_error(e: &Error) -> i32 {
    match e {
        Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_) => exit::RECORD,
        Error::Premise(_) => exit::PREMISE,
        Error::Unsettled(_) => exit::UNSETTLED,
        Error::Stopped
        | Error::Full
        | Error::Daemon(_)
        | Error::Zfs(_)
        | Error::Save(_)
        | Error::Io(_)
        | Error::NotUnderWay(_) => exit::ERROR,
    }
}

/// What [`execute`] does before it settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// Begin a publish.
    Publish,
    /// Nothing.
    Settle,
    /// Record a person's choice for a publish.
    Recover(String, Choice),
}

/// Run `task` on `agent`, then settle for at most `limit`: the
/// `unsettled` lines on `out`, errors on `err`; the exit code ([module
/// docs](self)). The outcome lines are the agent's outcome sink's, as
/// [`wired`] makes it.
pub fn execute<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    agent: &mut Agent<Z, D, K, C>,
    task: &Task,
    limit: Duration,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    if matches!(task, Task::Publish | Task::Settle) {
        if let Some(code) = refused_failed(agent, err) {
            return code;
        }
    }
    let mine = match task {
        Task::Publish => match agent.begin() {
            Ok(name) => Some(name),
            Err(e) => return failed(&e, err),
        },
        Task::Settle => None,
        Task::Recover(name, choice) => match agent.recover(name, *choice) {
            Ok(()) => Some(name.clone()),
            Err(e) => return failed(&e, err),
        },
    };
    // Each outcome line was printed as it was made, by the agent's
    // outcome sink ([`wired`]; S1 of #149's review).
    let settled = agent.settle(limit);
    let unsettled = match &settled {
        Ok(()) => Vec::new(),
        Err(Error::Unsettled(names)) => names.clone(),
        Err(e) => return failed(e, err),
    };
    for name in &unsettled {
        let _ = writeln!(out, "unsettled {name}");
    }
    let _ = out.flush();
    match mine {
        // By the name the publish goes by now: it may have taken the
        // next one (D2 of #149's review).
        Some(mine) => {
            let mine = agent.name_now(&mine);
            exit_of(
                agent
                    .outcomes()
                    .iter()
                    .rev()
                    .find(|(name, _)| *name == mine)
                    .map(|(_, outcome)| outcome),
            )
        }
        None => worst(agent.outcomes(), !unsettled.is_empty()),
    }
}

/// Whether `publish` or `settle` refuses for the volume's failure note
/// of this boot, and with what code, the words on `err`: [`exit::LOST`]
/// where the volume is lost, [`exit::ERROR`] where it failed with no
/// client holding its device, and the note's error where it cannot be
/// read. Neither touches the pool: only `run` does (the lead's answer
/// to Q5 of the plan of M2.7c). A daemon whose volume failed is gone, so
/// neither could publish; a start that acknowledges a failure no client
/// held removes the note.
fn refused_failed<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    agent: &mut Agent<Z, D, K, C>,
    err: &mut dyn Write,
) -> Option<i32> {
    match agent.failure() {
        Ok(Failed::No) => None,
        Ok(Failed::Unheld(failure)) => {
            let _ = writeln!(
                err,
                "skyzfs-agent: the volume failed in this boot ({}), and no client held its \
                 device: nothing is published; a start of the daemon with \
                 --acknowledge-failure serves it afresh",
                failure.reason
            );
            Some(exit::ERROR)
        }
        Ok(Failed::Lost(note)) => {
            let _ = writeln!(
                err,
                "skyzfs-agent: the volume is lost for this boot (note={} reason={} held={}): \
                 nothing is published; `run` makes its pool read",
                note_word(&note),
                note.reason_word(),
                note.held_word()
            );
            Some(exit::LOST)
        }
        Err(e) => Some(failed(&e, err)),
    }
}

/// `whole` or `torn`.
fn note_word(note: &Note) -> &'static str {
    match note {
        Note::Whole(_) => "whole",
        Note::Torn => "torn",
    }
}

/// Say on `err` what the round's step on a lost note showed, where it
/// is not the volume lost: the error of the reopen, and what the round
/// does next (the lead's rulings on F2 and F4 of the review of #173).
fn lost_said(n: u64, note: &Note, step: Option<&Step>, verdict: Verdict, err: &mut dyn Write) {
    if let Some(Step::Reopened {
        error: Some(why), ..
    }) = step
    {
        let _ = writeln!(
            err,
            "skyzfs-agent: round {n}: the reopen: {why} (the pool's state, not the command, \
             says what it did)"
        );
    }
    if let Some(Step::Reopened {
        unread: Some(why), ..
    }) = step
    {
        let _ = writeln!(
            err,
            "skyzfs-agent: round {n}: the look at the pool's state after the reopen: {why}"
        );
    }
    match verdict {
        Verdict::Lost => {}
        Verdict::Failed => {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: the volume failed in this boot (note={} reason={} \
                 held={}), and the pool is not imported: nothing is suspended; run goes on",
                note_word(note),
                note.reason_word(),
                note.held_word()
            );
        }
        Verdict::Retry => {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: the volume is lost for this boot (note={} reason={} \
                 held={}), and the pool was not shown suspended: the next round tries again",
                note_word(note),
                note.reason_word(),
                note.held_word()
            );
        }
    }
}

/// The line of standard output for a volume lost in this boot, before
/// the round line of the round that found it ([module docs](self),
/// "`run`"):
///
/// ```text
/// lost <pool> volume=<hex> note=whole|torn reason=<word|-> held=yes|unknown
///   step=not-imported|suspended|reopened|held|failed reopen=ok|failed|- state=<STATE|-|unread>
///   looked_ms=<ms|-> verdict=lost|failed|retry
/// ```
///
/// on one line. `step` is `None` where the step failed before the
/// reopen (its error is on standard error). `state=unread` where a look
/// after the reopen failed; `step=held` where `run`'s cap on reopens
/// ([`crate::engine::REOPEN_CAP`]) held the reopen back and the pool
/// read `state`; `verdict` is [`Step::verdict`].
pub fn lost_line(pool: &str, volume: &[u8; 16], note: &Note, step: Option<&Step>) -> String {
    let verdict = Step::verdict(step).word();
    let (word, reopen, state, looked) = match step {
        None => ("failed", "-", "-".to_string(), "-".to_string()),
        Some(Step::Reopened {
            error,
            state,
            unread,
            looked_ms,
        }) => (
            "reopened",
            if error.is_some() { "failed" } else { "ok" },
            if unread.is_some() {
                "unread".to_string()
            } else {
                state.as_ref().map_or("-".to_string(), ToString::to_string)
            },
            looked_ms.to_string(),
        ),
        Some(Step::Suspended) => ("suspended", "-", "SUSPENDED".to_string(), "-".to_string()),
        Some(Step::Held { state }) => ("held", "-", state.to_string(), "-".to_string()),
        Some(step) => (step.word(), "-", "-".to_string(), "-".to_string()),
    };
    format!(
        "lost {pool} volume={} note={} reason={} held={} step={word} reopen={reopen} \
         state={state} looked_ms={looked} verdict={verdict}\n",
        hex(volume),
        note_word(note),
        note.reason_word(),
        note.held_word(),
    )
}

/// Say `e` on `err`; its exit code.
fn failed(e: &Error, err: &mut dyn Write) -> i32 {
    let _ = writeln!(err, "skyzfs-agent: {e}");
    exit_of_error(e)
}

/// The volume's id from its file `remote`, in the volume's directory
/// ([`volume_id`]), or what is wrong, naming the file.
pub fn volume_of(run: &Run) -> Result<[u8; 16], String> {
    let remote = run.volume.join("remote");
    match std::fs::read(&remote) {
        Ok(bytes) => volume_id(&bytes).map_err(|why| format!("{}: {why}", remote.display())),
        Err(e) => Err(format!(
            "{}: {e} (a volume with no place in object storage publishes nothing)",
            remote.display()
        )),
    }
}

/// The agent of `volume` on its adapters, as `run` sets it: its settings
/// ([`configured`]), each report on `err` as a line ([`report_line`]) and
/// each outcome on `out` ([`outcome_line`]), each as it is made and
/// flushed. Everything the program decides before it acts, so that the
/// engine's fakes test it (S8 and T2 of #149's review); the binary only
/// makes the adapters of the machine.
pub fn wired<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    run: &Run,
    volume: [u8; 16],
    (zfs, daemon, keep, clock): (Z, D, K, C),
    mut out: Box<dyn Write>,
    mut err: Box<dyn Write>,
) -> Result<Agent<Z, D, K, C>, Error> {
    let agent = Agent::new(zfs, daemon, keep, clock, volume, &run.pool)?;
    let mut agent = configured(agent, run).with_sink(Box::new(move |name, report| {
        let _ = err.write_all(report_line(name, report).as_bytes());
        let _ = err.flush();
    }));
    agent.set_outcome_sink(Box::new(move |name, outcome| {
        let _ = out.write_all(outcome_line(name, outcome).as_bytes());
        let _ = out.flush();
    }));
    Ok(agent)
}

/// Whether `e` ends `run`: the record is corrupt or another volume's, or
/// the sweep finds no agent's marker. A person judges those, and a round
/// would find them again ([`exit::RECORD`]). Every other error fails its
/// round only.
fn ends_run(e: &Error) -> bool {
    matches!(e, Error::Corrupt(_) | Error::Volume(_) | Error::Unmarked(_))
}

/// Whether `run` starts the engine again after `e` ([`Agent::restart`]):
/// at once after an error of the syncs after a rewind import or of a
/// begin, before the round settles, and at the next round after a
/// settle's error ([`schedule`]); after any error but the two refusals
/// that [`Agent::begin`] makes before it changes anything, 8 publishes
/// under way ([`Error::Full`]) and the pool's premise
/// ([`Error::Premise`]).
/// Before this package an engine call that failed ended the program,
/// and the next one started from the record; within one process the
/// engine goes on from such an error only by the same way, so that no
/// path is new to the crash matrix.
fn restarts(e: &Error) -> bool {
    !matches!(e, Error::Full | Error::Premise(_))
}

/// What one round of `run` did ([`round_line`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    /// Its number, from 1.
    pub n: u64,
    /// Whether an engine call of the round failed (its error is on
    /// standard error).
    pub failed: bool,
    /// How long after its tick it began, in milliseconds.
    pub late_ms: u64,
    /// How long it took, in milliseconds.
    pub took_ms: u64,
    /// The ticks that passed while it ran and that get no round of their
    /// own.
    pub skipped: u64,
    /// The name of the publish it began; `None` where it began none.
    pub begun: Option<String>,
    /// The publishes under way when it ended.
    pub under_way: usize,
    /// The syncs it ran to pass the groups of a history a rewind import
    /// abandoned ([`Agent::sync_past`]).
    pub synced: u32,
    /// Whether its syncs met their bound with the daemon's newest cut
    /// still behind ([`crate::engine::Stuck::Bound`]).
    pub bound: bool,
    /// How long before its tick it began, in milliseconds, where it began
    /// early: on news of a lost volume, or as a quick retry of a step
    /// ([`QUICK_RETRY`]). `None` for a round on its tick or after it.
    pub early_ms: Option<u64>,
    /// Whether the cap on reopens ([`REOPEN_CAP`]) held the round's
    /// reopen back (`step=held` on its `lost` line).
    pub reopen_held: bool,
}

/// The line of standard output for a round of `run` (M2.T4 reads it):
///
/// ```text
/// round <n> ok|failed took_ms=<ms> late_ms=<ms> skipped=<ticks> begun=<t|-> under_way=<count>
///   synced=<k> bound=0|1 [early_ms=<ms>] [reopen=held]
/// ```
///
/// on one line. `early_ms` only on a round begun before its tick
/// ([`Round::early_ms`]), and `reopen=held` only on one whose reopen the
/// cap held back ([`Round::reopen_held`]): a round on its tick, of a
/// volume not lost, has the line it had before them.
pub fn round_line(round: &Round) -> String {
    let early = round
        .early_ms
        .map_or(String::new(), |ms| format!(" early_ms={ms}"));
    let held = if round.reopen_held {
        " reopen=held"
    } else {
        ""
    };
    format!(
        "round {} {} took_ms={} late_ms={} skipped={} begun={} under_way={} synced={} \
         bound={}{early}{held}\n",
        round.n,
        if round.failed { "failed" } else { "ok" },
        round.took_ms,
        round.late_ms,
        round.skipped,
        round.begun.as_deref().unwrap_or("-"),
        round.under_way,
        round.synced,
        u8::from(round.bound)
    )
}

/// `skyzfs-agent run`: a round at each tick, `every` apart on the clock
/// that does not step, the first at once, until `go` says no; the
/// exit code ([module docs](self), "`run`").
///
/// A round syncs the pool past an abandoned history
/// ([`Agent::sync_past`]; module docs, "After a rewind import"), begins
/// a publish ([`Agent::begin`]) where that left nothing behind, then
/// settles every publish under way ([`Agent::settle_while`]) until the
/// next tick, and at least one round of the engine: what is still under
/// way then is settled by the rounds after. A begin that fails (8 under
/// way, the pool's premise refused, no cuts from the daemon, ZFS) does
/// not keep the round from settling. The round's line follows on `out`
/// ([`round_line`]), each error of the round on `err`.
///
/// - **No overlap.** A round that runs past the next tick is not cut
///   short. Where ticks passed while it ran, the next round begins as it
///   ends, late, for the last of them, and the others are counted as
///   skipped in the line of the round that ran past them.
/// - **A failed round**, where an engine call failed, is reported, and
///   the next tick tries again. Where the error may have left the engine
///   unlike what it saved (`restarts`), the engine is loaded anew
///   ([`Agent::restart`]) before it is asked anything more, so that it
///   goes on as a new process would after a kill at that point: at once
///   where the syncs or the begin failed so, before the round settles
///   (a round whose syncs failed begins nothing), and at the next
///   round's beginning where the settling did. A restart that fails
///   is tried again at the next round, which settles nothing until it
///   succeeds. An error that ends `run`
///   (`ends_run`) ends it at once with [`exit::RECORD`].
/// - **An end** is asked of `go` before each round, before each sync,
///   before a begin that follows syncs, between the engine's rounds
///   within it, and every [`WAKE`] while it waits for a tick; it
///   ends `run` with [`exit::PUBLISHED`], with what is under way left in
///   the record for the next start.
/// - **The failure note** is read at each round's start; a note that
///   makes the volume lost has the round take the step and nothing else
///   ([module docs](self), "A volume that failed in this boot"). Between
///   round starts it is watched ([`Agent::watch_note`]): where an end is
///   asked of `go` within the engine, and every [`WAKE`] while it waits,
///   at most once a second. News ends the round there and begins the
///   next at once.
/// - **Retention** keeps what `kept` says ([`Kept`]; G16 of the review
///   of #179: the settings are taken here, not left to the caller).
/// - **What it keeps** of the publishes that left the record is
///   forgotten after each round ([`Agent::forget`]), once their lines
///   are out.
pub fn schedule<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    agent: &mut Agent<Z, D, K, C>,
    every: Duration,
    kept: Kept,
    go: &mut dyn FnMut() -> bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    agent.set_keep(kept.keep);
    agent.set_abandoned_keep(kept.abandoned_keep);
    let every_ms = ms(every).max(1);
    let start = agent.clock().mono_ms();
    let mut tick = 0u64;
    let mut restart = false;
    // The lost note the last steps were on, and what they did with it:
    // the reopens that left the pool unsuspended, the rounds since those
    // met the cap, and the quick retries in a row (the lead's rulings on
    // the review of #173). A new note starts them again.
    let mut noted: Option<Note> = None;
    let mut reopens = 0u64;
    let mut since_cap = 0u64;
    let mut quick = 0u32;
    for n in 1u64.. {
        if !go() {
            break;
        }
        let due = start.saturating_add(tick.saturating_mul(every_ms));
        let began = agent.clock().mono_ms();
        let mut failed = Vec::new();
        let mut begun = None;
        let mut ended = None;
        let mut synced = 0;
        let mut bound = false;
        // The volume's failure note of this boot (M2.7c), before anything
        // else of the round. One that failed with no client holding its
        // device is said, and the round touches nothing else (its daemon
        // is gone, and a start that acknowledges the failure serves it
        // afresh and removes the note). A
        // note that makes the volume lost: the round takes the step on
        // the pool and nothing else, and its verdict says what follows
        // (F2 and F4 of the review of #173): its `lost` line, then its
        // round line.
        let mut lost = None;
        let mut reopen_held = false;
        let read = agent.failure();
        match &read {
            Ok(Failed::No) => {}
            Ok(Failed::Unheld(failure)) => {
                let _ = writeln!(
                    err,
                    "skyzfs-agent: round {n}: the volume failed in this boot ({}), and no \
                     client held its device: no pool is touched; a start of the daemon with \
                     --acknowledge-failure serves it afresh",
                    failure.reason
                );
            }
            Ok(Failed::Lost(note)) => {
                if noted != Some(*note) {
                    noted = Some(*note);
                    reopens = 0;
                    since_cap = 0;
                    quick = 0;
                }
                // Past the cap, the pool is reopened only at every
                // REOPEN_CAPth round, and looked at in between.
                let reopen = if reopens >= REOPEN_CAP {
                    since_cap = since_cap.saturating_add(1);
                    since_cap.is_multiple_of(REOPEN_CAP)
                } else {
                    true
                };
                let taken = if reopen {
                    agent.lose()
                } else {
                    agent.look_lost()
                };
                let step = match taken {
                    Ok(step) => Some(step),
                    Err(e) => {
                        failed.push(e);
                        None
                    }
                };
                let verdict = Step::verdict(step.as_ref());
                reopen_held = matches!(step, Some(Step::Held { .. }));
                if verdict == Verdict::Retry && matches!(step, Some(Step::Reopened { .. })) {
                    reopens = reopens.saturating_add(1);
                    if reopens == REOPEN_CAP {
                        let _ = writeln!(
                            err,
                            "skyzfs-agent: round {n}: {REOPEN_CAP} reopens of this note left \
                             the pool unsuspended: it is reopened only every {REOPEN_CAP}th \
                             round from now on, and its state read at the others (is --pool \
                             the pool on this volume?)"
                        );
                    }
                }
                lost_said(n, note, step.as_ref(), verdict, err);
                lost = Some((
                    lost_line(agent.pool(), &agent.volume(), note, step.as_ref()),
                    verdict,
                ));
            }
            Err(_) => {}
        }
        let lost_verdict = lost.as_ref().map(|(_, verdict)| *verdict);
        // The watch on the note while the engine works and while `run`
        // waits (F1 of the review of #173): news is a lost note other
        // than this one.
        agent.watch_note(read.as_ref().ok().copied());
        // A note that cannot be read may be a lost volume's: the round
        // fails and touches nothing, as `publish` refuses (a nit of the
        // review of #173). The next round reads it again.
        let unread = read.is_err();
        let unheld = matches!(read, Ok(Failed::Unheld(_)));
        if let Err(e) = read {
            failed.push(e);
        }
        // The step's round touches nothing else, and nor does a round on a
        // volume that failed with no client: no pool is touched there (the
        // lead's answer to Q2; Codex's P2 on #173). Its next round reads the
        // note again, which a start with `--acknowledge-failure` removes.
        if lost.is_none() && !unread && !unheld {
            if restart {
                match agent.restart() {
                    Ok(()) => restart = false,
                    Err(e) => failed.push(e),
                }
            }
            // The pool synced past the groups of a history a rewind import
            // abandoned (M2.7d), and nothing begun while the daemon's newest
            // cut still does not open as itself: no cut could carry it.
            let mut clear = false;
            if !restart {
                match agent.sync_past(go) {
                    Ok(Past::Clear { synced: k }) => {
                        // An end asked during the syncs begins nothing: the
                        // begin would be a new step after it (Astra's P2 on
                        // #170). The settle below then ends at once.
                        synced = k;
                        clear = k == 0 || go();
                    }
                    Ok(Past::Unseen { synced: k }) => {
                        // The daemon went unread, perhaps after syncs that
                        // returned (L3 of the review of #172): the line
                        // counts them, and an end asked during the last
                        // begins nothing, as above. A round that synced
                        // nothing begins, and its begin says what is wrong.
                        synced = k;
                        clear = k == 0 || go();
                    }
                    Ok(Past::Behind {
                        synced: k,
                        group,
                        why,
                    }) => {
                        synced = k;
                        bound = why == Stuck::Bound;
                        let _ = writeln!(
                            err,
                            "skyzfs-agent: round {n}: no publish begun: the newest cut, of group \
                         {group}, does not open as itself: {why}"
                        );
                    }
                    Err(e) => {
                        synced = agent.last_synced();
                        // As a begin's error ([`restarts`]); the syncs change
                        // nothing that the engine saves, so a restart only
                        // costs a reading of the record.
                        let reload = restarts(&e);
                        failed.push(e);
                        if reload {
                            if let Err(again) = agent.restart() {
                                failed.push(again);
                                restart = true;
                            }
                        }
                    }
                }
            }
            // The watch asked once more before a begin: `sync_past` can
            // return after a sync or an unanswered `cuts` with no look at
            // the note, and a begin's count, sweep and reads would delay
            // the step (Codex's P1 on #173). News leaves the settle below
            // to end at once, and the next round begins with the step.
            if !restart && clear && !agent.note_came() {
                match agent.begin() {
                    Ok(name) => begun = Some(name),
                    Err(e) => {
                        // A begin that may have left the engine unlike what it
                        // saved (a save whose rename landed) restarts at once,
                        // before the engine settles anything: a save of the
                        // publishes under way from its own copy would write the
                        // new intent out of the record (P2 of Codex on #167).
                        // A refusal (`Full`, `Premise`) changed nothing.
                        let reload = restarts(&e);
                        failed.push(e);
                        if reload {
                            if let Err(again) = agent.restart() {
                                failed.push(again);
                                restart = true;
                            }
                        }
                    }
                }
            }
            // Retention (M2.7b-2): the names published past `--keep`, and
            // those given up past `--abandoned-keep-s`, retired, a few at a
            // round. It asks `go` and the note before each step and each
            // call to ZFS, and yields once an end was asked or the note
            // had news: the next round reads it first.
            if !restart {
                if let Err(e) = agent.retain_while(go) {
                    restart = restarts(&e);
                    failed.push(e);
                }
            }
            if !restart {
                let next = due.saturating_add(every_ms);
                let left = next.saturating_sub(agent.clock().mono_ms());
                let limit = Duration::from_millis(left).max(crate::engine::TICK);
                match agent.settle_while(limit, go) {
                    Ok(()) | Err(Error::Unsettled(_)) => {}
                    Err(e) => {
                        restart = restarts(&e);
                        failed.push(e);
                    }
                }
            }
        }
        for e in &failed {
            let _ = writeln!(err, "skyzfs-agent: round {n}: {e}");
            if ends_run(e) {
                ended = Some(exit_of_error(e));
            }
        }
        let now = agent.clock().mono_ms();
        let passed = now.saturating_sub(due) / every_ms;
        let round = Round {
            n,
            failed: !failed.is_empty() || lost_verdict == Some(Verdict::Retry),
            late_ms: began.saturating_sub(due),
            took_ms: now.saturating_sub(began),
            skipped: passed.saturating_sub(1),
            begun,
            under_way: agent.record().under_way.len(),
            synced,
            bound,
            early_ms: (began < due).then(|| due - began),
            reopen_held,
        };
        if let Some((line, _)) = &lost {
            let _ = out.write_all(line.as_bytes());
        }
        let _ = out.write_all(round_line(&round).as_bytes());
        let _ = out.flush();
        if lost_verdict == Some(Verdict::Lost) {
            return exit::LOST;
        }
        if let Some(code) = ended {
            return code;
        }
        if let Some(why) = agent.moved() {
            let _ = writeln!(
                err,
                "skyzfs-agent: round {n}: {why}: run ends, to be started again on the device \
                 as it is now"
            );
            return exit::MOVED;
        }
        agent.forget();
        // A round begun early does not use up its tick: the round of that
        // tick still comes on it (D1 of the review of #173).
        let early = began < due;
        tick = tick.saturating_add(if early { passed } else { passed.max(1) });
        let due = start.saturating_add(tick.saturating_mul(every_ms));
        // A step whose verdict is retry is taken again within seconds, a
        // few times in a row ([`QUICK_RETRY`], [`QUICK_RETRIES`]); then at
        // the ticks. Not past the cap on reopens.
        let mut until = due;
        if lost_verdict == Some(Verdict::Retry) {
            if quick < QUICK_RETRIES && reopens < REOPEN_CAP {
                quick += 1;
                until = until.min(now.saturating_add(ms(QUICK_RETRY)));
            }
        } else {
            quick = 0;
        }
        // News on the note begins the next round at once, early for its
        // tick.
        while agent.clock().mono_ms() < until && !agent.note_came() {
            if !go() {
                return exit::PUBLISHED;
            }
            let left = until.saturating_sub(agent.clock().mono_ms());
            agent.clock().sleep(Duration::from_millis(left).min(WAKE));
        }
    }
    exit::PUBLISHED
}

/// `d` in milliseconds.
fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The volume's id from the bytes of its file `remote` ([module
/// docs](self)): the ULID of its line `id`, as 16 bytes, most
/// significant first.
pub fn volume_id(remote: &[u8]) -> Result<[u8; 16], String> {
    let text = std::str::from_utf8(remote).map_err(|_| "not UTF-8".to_string())?;
    let body = text
        .strip_suffix('\n')
        .ok_or("its last line has no line feed")?;
    let lines: Vec<&str> = body.split('\n').collect();
    let keys = ["endpoint", "bucket", "volume", "id"];
    match lines.split_first() {
        Some((&"skyzfs-remote 1", rest)) if rest.len() == keys.len() => {
            let mut id = "";
            for (line, key) in rest.iter().zip(keys) {
                match line.strip_prefix(key).and_then(|v| v.strip_prefix(' ')) {
                    Some(value) if !value.is_empty() => id = value,
                    _ => return Err(format!("{line:?} where the line {key:?} belongs")),
                }
            }
            ulid(id).map(u128::to_be_bytes)
        }
        Some((first, _)) if *first != "skyzfs-remote 1" => Err(format!(
            "its first line is {first:?}, not \"skyzfs-remote 1\""
        )),
        _ => Err("not the five lines of \"skyzfs-remote 1\"".into()),
    }
}

/// A ULID's 26 characters of Crockford's base 32 (upper or lower case),
/// as its 128 bits.
fn ulid(text: &str) -> Result<u128, String> {
    const DIGITS: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bad = || format!("the id {text:?} is not a ULID");
    if text.len() != 26 {
        return Err(bad());
    }
    let mut value: u128 = 0;
    for (i, c) in text.bytes().enumerate() {
        let digit = DIGITS
            .iter()
            .position(|&d| d == c.to_ascii_uppercase())
            .ok_or_else(bad)?;
        // The first character holds 3 bits: 26 × 5 = 130.
        if i == 0 && digit > 7 {
            return Err(bad());
        }
        value = (value << 5) | u128::try_from(digit).map_err(|_| bad())?;
    }
    Ok(value)
}

/// The record as `skyzfs-agent record` prints it (testing's item 4), or
/// why it is not read. One line a fact, its words between single spaces,
/// `key=value` after the first two; `-` where there is no value:
///
/// ```text
/// volume <32 hex digits>
/// enrolled <pool> pool_guid=<n> leaf_guid=<n>      or: enrolled -
/// publish <t> phase=<intent|taken|held|asked|done|abandoning> count=<C> group=<G>
///   stalls=<n> attempts=<n> held=<yes|no> asked_ms=<ms> held_until_ms=<ms>
///   manifest=<64 hex digits> commit=<64 hex digits> person=<-|stopped|give-up>
///   begun_ms=<ms>                                   (one line)
/// abandoned <t> why=<word> at_ms=<ms> left=<nothing|unknown|members:<n>>
/// published <t> commit=<64 hex digits> group=<G> members=<n> at_ms=<ms>
///   retire=<-|ref|members|other>                    (one line)
/// stuck <t> members=<n> at_ms=<ms>
/// ```
///
/// `left` and the lines `published` and `stuck` are version 3's
/// (M2.7b-2): what a name given up left on the pool, the names published
/// that retention keeps, and the names whose destroy failed.
///
/// With no record, the one line `absent`.
pub fn dump(loaded: &Loaded) -> Result<String, String> {
    let record = match loaded {
        Loaded::Absent => return Ok("absent\n".into()),
        Loaded::Corrupt(why) => return Err(format!("the record is corrupt: {why}")),
        Loaded::Present(record) => record,
    };
    let mut out = format!("volume {}\n", hex(&record.volume));
    match &record.enrolled {
        None => out.push_str("enrolled -\n"),
        Some(e) => out.push_str(&format!(
            "enrolled {} pool_guid={} leaf_guid={}\n",
            e.pool, e.pool_guid, e.leaf_guid
        )),
    }
    let dash = || "-".to_string();
    for p in &record.under_way {
        let (phase, taken, asked, until, commit) = match &p.phase {
            Phase::Intent => ("intent", None, None, None, None),
            Phase::Taken(t) => ("taken", Some(t), None, None, None),
            Phase::Held(t) => ("held", Some(t), None, None, None),
            Phase::Asked {
                taken,
                asked_ms,
                held_until_ms,
            } => (
                "asked",
                Some(taken),
                Some(*asked_ms),
                Some(*held_until_ms),
                None,
            ),
            Phase::Done { taken, commit } => ("done", Some(taken), None, None, Some(commit)),
            Phase::Abandoning { .. } => ("abandoning", None, None, None, None),
        };
        let person = match p.person {
            None => "-",
            Some(Person::Stopped) => "stopped",
            Some(Person::GiveUp) => "give-up",
        };
        out.push_str(&format!(
            "publish {} phase={phase} count={} group={} stalls={} attempts={} held={} \
             asked_ms={} held_until_ms={} manifest={} commit={} person={person} begun_ms={}\n",
            p.name,
            p.count.after,
            taken.map_or_else(dash, |t| t.group.to_string()),
            p.stalls,
            p.attempts,
            if p.held { "yes" } else { "no" },
            asked.map_or_else(dash, |ms| ms.to_string()),
            until.map_or_else(dash, |ms| ms.to_string()),
            taken.map_or_else(dash, |t| hex(&t.manifest_hash)),
            commit.map_or_else(dash, |c| hex(c)),
            p.begun_ms,
        ));
    }
    for a in &record.abandoned {
        let left = match &a.left {
            Left::Nothing => "nothing".to_string(),
            Left::Unknown => "unknown".to_string(),
            Left::Members(members) => format!("members:{}", members.len()),
        };
        out.push_str(&format!(
            "abandoned {} why={} at_ms={} left={left}\n",
            a.name,
            why_word(a.why),
            a.at_ms
        ));
    }
    for p in &record.published {
        let retire = match p.retire {
            None => "-",
            Some(Retire::Ref) => "ref",
            Some(Retire::Members) => "members",
            Some(Retire::Other) => "other",
        };
        out.push_str(&format!(
            "published {} commit={} group={} members={} at_ms={} retire={retire}\n",
            p.name,
            hex(&p.commit),
            p.group,
            p.members.len(),
            p.at_ms
        ));
    }
    for s in &record.stuck {
        out.push_str(&format!(
            "stuck {} members={} at_ms={}\n",
            s.name,
            s.members.len(),
            s.at_ms
        ));
    }
    Ok(out)
}

/// The agent's settings from `run`'s test hooks and expiry, on `agent`.
pub fn configured<Z: Zfs, D: Daemon, K: Keep, C: Clock>(
    mut agent: Agent<Z, D, K, C>,
    run: &Run,
) -> Agent<Z, D, K, C> {
    if let Some(expiry) = run.expiry {
        agent = agent.with_expiry(expiry);
    }
    if let Some(after) = run.hooks.assist_after {
        agent = agent.with_assist_after(after);
    }
    if let Some(age_max) = run.hooks.age_max {
        agent = agent.with_age_max(age_max);
    }
    agent
}

#[cfg(test)]
mod tests;
