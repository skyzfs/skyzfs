//! [`Zfs`] on the `zfs` and `zpool` commands ([`ZfsCli`]), run on a
//! [`Host`] with a time limit each (the plan of M2.6, §8, package c).
//!
//! # What each call runs
//!
//! - [`Zfs::layout`]: `zpool status -P POOL` and `zpool status -P -g
//!   POOL`. The first gives each vdev's name (a leaf's path, an interior
//!   vdev's kind) and the pool's scan and removal lines, the second each
//!   vdev's GUID in the same places; the two trees must have the same
//!   shape. A leaf's device number is read by `stat` of its path. A leaf
//!   that names no path, or whose path cannot be read, has none
//!   ([`Vdev::rdev`] `None`), which the premise refuses.
//! - [`Zfs::props`]: `zpool get -H -p -o property,value
//!   guid,load_guid,multihost,readonly,checkpoint POOL`. ZFS 2.2's
//!   `zpool get` says `missing pool name.` (exit 2) of a pool that does
//!   not exist, as of no pool named at all; then `zpool status -P POOL`
//!   is asked, and its `no such pool` gives [`ZfsError::NoPool`].
//! - [`Zfs::nocacheflush`]: the module parameter's file,
//!   [`NOCACHEFLUSH`].
//! - [`Zfs::snapshot`]: a channel program, `snapshot.lua` beside this
//!   file, on its standard input: `zfs program -j POOL - POOL T
//!   skyzfs:assist`.
//! - [`Zfs::members`] and [`Zfs::tagged`]: read-only channel programs
//!   (`zfs program -n -j`), `members.lua` and `tagged.lua`.
//! - [`Zfs::held`]: `zfs holds -H pool@t`.
//! - [`Zfs::hold`] and [`Zfs::release`]: `zfs hold -r` and `zfs release
//!   -r`. A hold that ZFS refuses because a member has the tag already
//!   held nothing (ZFS holds all or none): the members are read with the
//!   members program and their holds with `zfs holds -H -r pool@t`, and
//!   it is held only where every member's `@t` has the tag (F3 of the
//!   review of #147). A release that ZFS refuses for want of the tag is
//!   checked with `zfs holds -H -r pool@t`: no member may keep the tag.
//!   A message is matched on the command's standard error, on the words
//!   after a line's last `: `, so that a dataset's name is never read as
//!   one (S1).
//! - [`Zfs::assist`]: `zfs set skyzfs:assist=V POOL`, then `zpool sync
//!   POOL`.
//!
//! Every command runs in the locale `C`, so that its messages can be
//! read; `no such pool` gives [`ZfsError::NoPool`], `dataset does not
//! exist` [`ZfsError::NoSnapshot`] where a snapshot was named, and
//! anything else [`ZfsError::Other`] with the command and its message.
//! `zfs hold`, `holds` and `release` say `dataset does not exist` of a
//! pool that does not exist: they give [`ZfsError::NoSnapshot`], not
//! [`ZfsError::NoPool`] (recorded on both images).
//! A command past its limit is killed and not waited for ([`Host`]), and
//! gives [`ZfsError::Other`].
//!
//! # The channel program
//!
//! It walks the pool's file systems and volumes from its root, checks
//! that each member's `@t` can be taken, and reads every member's type,
//! GUID and locally set properties before it takes any; then it takes
//! them all, in one sync task and so in one transaction group, and reads
//! each `@t`'s GUID and `createtxg` of `pool@t`. A member's `@t` that
//! exists, or any check that fails, returns before anything is taken
//! ([`ZfsError::Exists`] for `EEXIST`). A property that cannot be read
//! raises, and fails the program before anything is taken: none is left
//! out of the manifest without a word. (The sandbox has no `pcall`, so
//! nothing is caught.)
//!
//! Some failures come after a take has begun, and leave some or all
//! members' `@t` taken: a take that fails on space (`ENOSPC`, each
//! check and take counting its own) or on a snapshot limit (`EDQUOT`),
//! a take that raises (`EIO`), the instruction limit reached in the take
//! loop or in the reads after it, and a returned value larger than the
//! memory limit. The program says so where it can (`taken`), and each
//! is [`ZfsError::Other`]. Where it says it took some (`taken` above 0)
//! and failed on any errno but `EEXIST`, `pool@t` is destroyed with what
//! it took, as for an output that cannot be read, and the error says
//! what became of it (the lead's decision on testing's scale run, where
//! 10000 file systems failed on `ENOSPC` after 4563 were taken; it
//! withdraws F9 of #147's first review for this case): a partial take
//! is never published, so nothing relies on it. Where it could not say
//! (an instruction or memory limit), the engine's next round finds
//! `pool@t` and gives the publish up as `ManifestLost`, the snapshots
//! kept for a person.
//!
//! The program's values are all strings (`tostring`), and its tables
//! are keyed by name: `zfs program -j` writes a Lua number as a signed
//! 64-bit one (a GUID above 2^63 negative) and a list as an object keyed
//! by position, so names and strings are what reads the same on every
//! version. A GUID is read as unsigned, or as signed and taken modulo
//! 2^64.
//!
//! A property's value that is not printable ASCII comes in hex: `{
//! "encoding": "hex", "value": "..." }`, lower case, as the manifest
//! writes it ([`crate::manifest`]). `zfs program -j` prints strings
//! through `mbrtowc`, which in the locale `C` stops at a byte above 0x7f:
//! it cuts the output there, and still exits 0 (B2 of the review of
//! #147). Names are ASCII by ZFS's rules; values are not.
//!
//! Output that cannot be read after the program exited 0 (cut off, not
//! UTF-8, or not a whole result and not a failure the program reported)
//! means that what it took cannot be known: `zfs destroy -r pool@t` is
//! run before anything relies on it, and the error says whether the
//! snapshots were destroyed, were not taken, or could not be destroyed
//! (the lead's L1 on #147).
//!
//! # What was checked on ZFS, and what was not
//!
//! The parsers are tested on output written by hand from OpenZFS's
//! sources (`tests.rs`) and on output recorded by testing's recorder on
//! ZFS 2.2 userland over a 2.4.1 module (Ubuntu 24.04) and on 2.4.1
//! (26.04), `cli/recorded/` (`recordings.rs`; the plan's Q10). The
//! recordings show:
//!
//! - `zfs program` reads a script named `-` from its standard input, and
//!   writes a returned GUID above 2^63 as a negative number.
//! - The members and tagged programs run and return what the parsers
//!   read, on the rig's pool and on a mirror of two files.
//! - The channel-program sandbox has no `pcall`: the snapshot program at
//!   `366b217` failed on it on both images before it took anything. It
//!   no longer calls it; the program as it is now has not run on ZFS.
//! - A checkpoint is none where `zpool get -p` gives `-`; one that exists
//!   gives its size.
//! - A resilver under way says `resilver in progress` on its `scan:`
//!   line; a scrub under way, paused or cancelled, and a resilver that
//!   has ended, do not make the pool busy.
//! - A log on its own line reads `logs` with a tab after it.
//! - `zfs hold -r` of a tag one member has already fails with `tag
//!   already exists` and holds nothing; `zfs release -r` of a tag no
//!   member has fails with `no such tag`.
//! - A pool that does not exist, for each command (above): `zpool` and
//!   `zfs program` say `no such pool`, `zfs hold`, `holds` and `release`
//!   say `dataset does not exist`.
//!
//! **Readings not checked on ZFS:**
//!
//! - "Set locally" is a property whose source, as `zfs.get_prop` and the
//!   user-property list give it, is the member itself. A received value
//!   is not local here.
//! - The program's limits are the module's defaults for the most a
//!   program may ask: 100 000 000 instructions and 100 MiB of memory
//!   ([`PROGRAM_INSTRUCTIONS`], [`PROGRAM_MEMORY`]). A pool with more
//!   members than that allows fails the program; it takes nothing only
//!   where the limit is reached before the take loop (above).
//! - A checkpoint is none where `zpool get -p` gives `0`.
//! - A rebuild under way: the `scan:` line names a rebuild, or a
//!   resilver with its vdev in brackets. A removal: any `remove:` line,
//!   under way or done, which the premise refuses (the lead's L4 on
//!   #147); one under way is busy too. `zpool status` never prints an
//!   indirect vdev.
//! - Property values that are not printable ASCII, in hex as above.
//! - The time limits ([`Limits`]): a read 60 s, a change 120 s. They are
//!   provisional.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{Layout, Program, Props, Vdev, VdevKind, Zfs, ZfsError, ASSIST_PROPERTY};
use crate::host::{Host, Node, Output};
use crate::manifest::{self, Kind};
use crate::record::Member;

/// The module parameter that says whether ZFS sends cache flushes.
pub const NOCACHEFLUSH: &str = "/sys/module/zfs/parameters/zfs_nocacheflush";

/// The channel program that takes the snapshots ([module docs](self)).
pub const SNAPSHOT_PROGRAM: &str = include_str!("cli/snapshot.lua");

/// The channel program that reads each member's `@t`.
pub const MEMBERS_PROGRAM: &str = include_str!("cli/members.lua");

/// The channel program that finds the snapshots with a hold.
pub const TAGGED_PROGRAM: &str = include_str!("cli/tagged.lua");

/// The most instructions a channel program is let run: the default of
/// `zfs_lua_max_instrlim`.
pub const PROGRAM_INSTRUCTIONS: u64 = 100_000_000;

/// The most memory a channel program is let use: the default of
/// `zfs_lua_max_memlimit`.
pub const PROGRAM_MEMORY: u64 = 100 << 20;

/// `EEXIST`, as a channel program's check gives it.
const EEXIST: i64 = 17;

/// How long a command may run before it is killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// A command that reads: `zpool status`, `zpool get`, `zfs holds`,
    /// a read-only program.
    pub read: Duration,
    /// A command that changes the pool and waits for a group to be
    /// written: the snapshot program, `zfs hold`, `zfs release`, `zfs
    /// set`, `zpool sync`.
    pub change: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            read: Duration::from_secs(60),
            change: Duration::from_secs(120),
        }
    }
}

/// [`Zfs`] on the commands ([module docs](self)).
#[derive(Debug)]
pub struct ZfsCli<H> {
    host: H,
    limits: Limits,
}

impl<H: Host> ZfsCli<H> {
    /// The commands on `host`, each within `limits`.
    pub fn new(host: H, limits: Limits) -> Self {
        ZfsCli { host, limits }
    }

    /// The host, for the tests.
    pub fn host(&mut self) -> &mut H {
        &mut self.host
    }

    /// Run `program` with `args` within `limit`: what it gave, whatever
    /// its exit code.
    fn exec(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
    ) -> Result<Output, ZfsError> {
        self.host
            .run(program, args, stdin, limit)
            .map_err(|e| ZfsError::Other(format!("{}: {e}", command(program, args))))
    }

    /// Run `program` with `args` within `limit`; its standard output
    /// where it exits 0, and otherwise the error its message says.
    fn run(
        &mut self,
        program: &str,
        args: &[&str],
        stdin: &[u8],
        limit: Duration,
        snapshot: bool,
    ) -> Result<String, ZfsError> {
        let output = self.exec(program, args, stdin, limit)?;
        if output.code == Some(0) {
            return String::from_utf8(output.stdout).map_err(|_| {
                ZfsError::Other(format!(
                    "{}: its output is not UTF-8",
                    command(program, args)
                ))
            });
        }
        Err(failure(program, args, &output, snapshot))
    }

    fn zpool(&mut self, args: &[&str], limit: Duration) -> Result<String, ZfsError> {
        self.run("zpool", args, &[], limit, false)
    }

    fn zfs(&mut self, args: &[&str], limit: Duration, snapshot: bool) -> Result<String, ZfsError> {
        self.run("zfs", args, &[], limit, snapshot)
    }

    /// A channel program's returned value, the object under `return`.
    fn program(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
    ) -> Result<serde_json::Map<String, Value>, ZfsError> {
        let out = self.program_output(pool, text, args, read_only)?;
        readable(&out).map_err(|why| ZfsError::Other(format!("zfs program: {why}")))
    }

    /// The snapshot program's output could not be read after it exited 0
    /// (`why`): `zfs destroy -r pool@t`. The error says what became of
    /// the snapshots. `zfs destroy -r` of a snapshot that is not there
    /// says `could not find any snapshots to destroy` (`zfs_main.c`; not
    /// recorded on ZFS), or `dataset does not exist` of a pool that is
    /// not; the second can also be a pool exported in between, so it
    /// reads "was not found". `zfs destroy` takes `%` ranges and `,`
    /// lists after the `@`: a `t` that is not a snapshot's name
    /// ([`skyzfs_control::valid_name`]) is not destroyed at all.
    fn destroy_taken(&mut self, pool: &str, t: &str, why: &str) -> ZfsError {
        let gone = self.destroy(pool, t);
        ZfsError::Other(format!(
            "zfs program (snapshot) exited 0 and its output cannot be read: {why}; {gone}"
        ))
    }

    /// `pool@t` destroyed with what a take that failed part way took
    /// (the lead's decision on testing's scale run for #147): `said` is
    /// what the program said, and the error says what became of it.
    fn destroy_partial(&mut self, pool: &str, t: &str, said: &str) -> ZfsError {
        let gone = self.destroy(pool, t);
        ZfsError::Other(format!("{said}; {gone}"))
    }

    /// `zfs destroy -r pool@t`, and what became of it, in words.
    fn destroy(&mut self, pool: &str, t: &str) -> String {
        let snapshot = format!("{pool}@{t}");
        let args = ["destroy", "-r", snapshot.as_str()];
        if !skyzfs_control::valid_name(t) {
            format!("{snapshot} was not destroyed: `{t}` is not a snapshot's name")
        } else {
            self.destroyed(&snapshot, &args)
        }
    }

    /// What became of `snapshot` by `zfs destroy -r` ([`Self::destroy_taken`]).
    fn destroyed(&mut self, snapshot: &str, args: &[&str]) -> String {
        match self.exec("zfs", args, &[], self.limits.change) {
            Ok(output) if output.code == Some(0) => format!("{snapshot} destroyed"),
            Ok(output) if says(&output.stderr, "could not find any snapshots to destroy") => {
                format!("{snapshot} was not taken")
            }
            Ok(output) => match failure("zfs", args, &output, true) {
                ZfsError::NoSnapshot => format!("{snapshot} was not found"),
                e => format!("{snapshot} could not be destroyed: {e}"),
            },
            Err(e) => format!("{snapshot} could not be destroyed: {e}"),
        }
    }

    /// The snapshots under `snapshot` (`pool@t` and its members' `@t`)
    /// that have a hold of `tag`, by `zfs holds -H -r`; `None` where
    /// `snapshot` does not exist.
    fn tagged_snapshots(
        &mut self,
        snapshot: &str,
        tag: &str,
    ) -> Result<Option<BTreeSet<String>>, ZfsError> {
        let holds = match self.zfs(&["holds", "-H", "-r", snapshot], self.limits.read, true) {
            Ok(holds) => holds,
            Err(ZfsError::NoSnapshot) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(
            holds
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split('\t');
                    let name = fields.next()?;
                    (fields.next() == Some(tag)).then(|| name.to_string())
                })
                .collect(),
        ))
    }

    /// A channel program's standard output, where it exits 0.
    fn program_output(
        &mut self,
        pool: &str,
        text: &str,
        args: &[&str],
        read_only: bool,
    ) -> Result<Vec<u8>, ZfsError> {
        let instructions = PROGRAM_INSTRUCTIONS.to_string();
        let memory = PROGRAM_MEMORY.to_string();
        let mut all = vec!["program"];
        if read_only {
            all.push("-n");
        }
        all.extend(["-j", "-t", &instructions, "-m", &memory, pool, "-"]);
        all.extend(args);
        let limit = if read_only {
            self.limits.read
        } else {
            self.limits.change
        };
        let output = self.exec("zfs", &all, text.as_bytes(), limit)?;
        if output.code == Some(0) {
            Ok(output.stdout)
        } else {
            Err(failure("zfs", &all, &output, false))
        }
    }

    /// The tree of `zpool status -P` (`guids` false) or `-P -g` (true).
    fn status(&mut self, pool: &str, guids: bool) -> Result<Status, ZfsError> {
        let mut args = vec!["status", "-P"];
        if guids {
            args.push("-g");
        }
        args.push(pool);
        let out = self.zpool(&args, self.limits.read)?;
        parse_status(&out, pool).map_err(|why| ZfsError::Other(format!("zpool status: {why}")))
    }

    /// A vdev of the `-P` tree, its GUID from the `-g` tree's node in the
    /// same place.
    fn vdev(&mut self, named: &Entry, guided: &Entry) -> Result<Vdev, ZfsError> {
        let guid = parse_guid(&guided.name).ok_or_else(|| {
            ZfsError::Other(format!("zpool status -g: `{}` is not a GUID", guided.name))
        })?;
        let mut children = Vec::with_capacity(named.children.len());
        for (n, g) in named.children.iter().zip(&guided.children) {
            children.push(self.vdev(n, g)?);
        }
        let (kind, rdev) = match interior(&named.name) {
            Some(kind) => (kind, None),
            None if !children.is_empty() => (VdevKind::Other(named.name.clone()), None),
            None if named.name.starts_with('/') => {
                match self.host.stat(Path::new(&named.name)) {
                    Ok(Node::Block(rdev)) => (VdevKind::Disk, Some(rdev)),
                    Ok(Node::File) => (VdevKind::File, None),
                    // A path that is neither, or cannot be read: a disk
                    // whose device is not known, which the premise refuses.
                    Ok(Node::Other) | Err(_) => (VdevKind::Disk, None),
                }
            }
            // No path: a leaf ZFS cannot open, named by its GUID.
            None => (VdevKind::Other(named.name.clone()), None),
        };
        Ok(Vdev {
            kind,
            guid,
            rdev,
            children,
        })
    }
}

impl<H: Host> Zfs for ZfsCli<H> {
    fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError> {
        let named = self.status(pool, false)?;
        let guided = self.status(pool, true)?;
        if !named.same_shape(&guided) {
            return Err(ZfsError::Other(
                "zpool status: the pool's vdevs changed between two reads".into(),
            ));
        }
        let mut layout = Layout {
            data: Vec::new(),
            special: 0,
            dedup: 0,
            other: 0,
            busy: named.busy.clone(),
            removed: named.removed.clone(),
        };
        for ((class, named), (_, guided)) in named.classes.iter().zip(&guided.classes) {
            match class {
                Class::Data => {
                    for (n, g) in named.iter().zip(guided) {
                        let vdev = self.vdev(n, g)?;
                        layout.data.push(vdev);
                    }
                }
                Class::Special => layout.special += named.len(),
                Class::Dedup => layout.dedup += named.len(),
                Class::Logs | Class::Cache | Class::Spares => layout.other += named.len(),
            }
        }
        Ok(layout)
    }

    fn props(&mut self, pool: &str) -> Result<Props, ZfsError> {
        let args = [
            "get",
            "-H",
            "-p",
            "-o",
            "property,value",
            "guid,load_guid,multihost,readonly,checkpoint",
            pool,
        ];
        let output = self.exec("zpool", &args, &[], self.limits.read)?;
        if output.code != Some(0) {
            let error = failure("zpool", &args, &output, false);
            // ZFS 2.2's `zpool get` takes a name that is no pool's for no
            // pool named at all ([module docs](self)): `zpool status`
            // says whether the pool is there.
            if matches!(error, ZfsError::Other(_)) && says(&output.stderr, "missing pool name") {
                if let Err(ZfsError::NoPool) = self.zpool(&["status", "-P", pool], self.limits.read)
                {
                    return Err(ZfsError::NoPool);
                }
            }
            return Err(error);
        }
        let out = String::from_utf8(output.stdout)
            .map_err(|_| ZfsError::Other("zpool get: its output is not UTF-8".into()))?;
        parse_props(&out).map_err(|why| ZfsError::Other(format!("zpool get: {why}")))
    }

    fn nocacheflush(&mut self) -> Result<bool, ZfsError> {
        let bytes = self
            .host
            .read(Path::new(NOCACHEFLUSH))
            .map_err(|e| ZfsError::Other(format!("{NOCACHEFLUSH}: {e}")))?;
        let text = String::from_utf8_lossy(&bytes);
        match text.trim().parse::<i64>() {
            Ok(n) => Ok(n != 0),
            Err(_) => Err(ZfsError::Other(format!(
                "{NOCACHEFLUSH}: `{}` is not a number",
                text.trim()
            ))),
        }
    }

    fn snapshot(&mut self, pool: &str, t: &str) -> Result<super::Program, ZfsError> {
        let out =
            self.program_output(pool, SNAPSHOT_PROGRAM, &[pool, t, ASSIST_PROPERTY], false)?;
        // It exited 0, so it ran to its end. Where its output cannot be
        // read, or does not say it failed and is not a whole result, what
        // it took cannot be known ([module docs](self)): `pool@t` is
        // destroyed with whatever it took, before anything relies on it.
        let returned = match readable(&out) {
            Ok(returned) => returned,
            Err(why) => return Err(self.destroy_taken(pool, t, &why)),
        };
        match parse_program(&returned, pool) {
            Err(ZfsError::Other(why)) if !returned.contains_key("failed") => {
                // The error already names the program once.
                let why = why.strip_prefix(PROGRAM_SAYS).unwrap_or(&why);
                Err(self.destroy_taken(pool, t, why))
            }
            Err(ZfsError::Other(said)) if partly_taken(&returned) => {
                Err(self.destroy_partial(pool, t, &said))
            }
            done => done,
        }
    }

    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
        let returned = self.program(pool, MEMBERS_PROGRAM, &[pool, t], true)?;
        parse_members(&returned, pool)
            .map_err(|why| ZfsError::Other(format!("zfs program (members): {why}")))
    }

    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        let args = ["hold", "-r", tag, &snapshot];
        let output = self.exec("zfs", &args, &[], self.limits.change)?;
        if output.code == Some(0) {
            return Ok(());
        }
        if !says(&output.stderr, "tag already exists") {
            return Err(failure("zfs", &args, &output, true));
        }
        // A member has the tag, and so the command held none (ZFS holds
        // all or none): the hold is there only where every member's `@t`
        // has it.
        let Some(members) = self.members(pool, t)? else {
            return Err(ZfsError::NoSnapshot);
        };
        let tagged = self.tagged_snapshots(&snapshot, tag)?.unwrap_or_default();
        let lacking: Vec<String> = members
            .iter()
            .map(|m| format!("{}@{t}", m.name))
            .filter(|name| !tagged.contains(name))
            .collect();
        if lacking.is_empty() {
            Ok(())
        } else {
            Err(ZfsError::Other(format!(
                "zfs hold -r {tag} {snapshot}: the tag is on some members and not on {}",
                lacking.join(", ")
            )))
        }
    }

    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        let args = ["release", "-r", tag, &snapshot];
        let output = self.exec("zfs", &args, &[], self.limits.change)?;
        if output.code == Some(0) {
            return Ok(());
        }
        if !says(&output.stderr, "no such tag") {
            return match failure("zfs", &args, &output, true) {
                ZfsError::NoSnapshot => Ok(()),
                e => Err(e),
            };
        }
        // `-r` releases what has the tag and refuses for what has not:
        // that no member keeps it is checked.
        let Some(tagged) = self.tagged_snapshots(&snapshot, tag)? else {
            return Ok(());
        };
        if tagged.is_empty() {
            Ok(())
        } else {
            let kept: Vec<&str> = tagged.iter().map(String::as_str).collect();
            Err(ZfsError::Other(format!(
                "zfs release -r {tag} {snapshot}: {} kept the hold: {}",
                kept.join(", "),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn held(&mut self, pool: &str, t: &str, tag: &str) -> Result<bool, ZfsError> {
        // Every member's `@t`, not only the root's: a hold on some members
        // only (made by hand, or by a `hold -r` that a member's snapshot
        // came after) is not held, so that the planner holds again and
        // the hold's check refuses it (NL-F1 of #147's review).
        let snapshot = format!("{pool}@{t}");
        let Some(tagged) = self.tagged_snapshots(&snapshot, tag)? else {
            return Ok(false);
        };
        if !tagged.contains(&snapshot) {
            return Ok(false);
        }
        let Some(members) = self.members(pool, t)? else {
            return Ok(false);
        };
        Ok(members
            .iter()
            .all(|m| tagged.contains(&format!("{}@{t}", m.name))))
    }

    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError> {
        let returned = self.program(pool, TAGGED_PROGRAM, &[pool, tag], true)?;
        match returned.get("tagged") {
            Some(Value::Object(names)) => Ok(names.keys().cloned().collect()),
            // An empty table: ZFS may write it as an empty list.
            Some(Value::Array(names)) if names.is_empty() => Ok(BTreeSet::new()),
            _ => Err(ZfsError::Other(
                "zfs program (tagged): no `tagged` object returned".into(),
            )),
        }
    }

    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError> {
        let set = format!("{ASSIST_PROPERTY}={value}");
        self.zfs(&["set", &set, pool], self.limits.change, false)?;
        self.zpool(&["sync", pool], self.limits.change)?;
        Ok(())
    }
}

/// `program args...`, as a person would type it.
fn command(program: &str, args: &[&str]) -> String {
    let mut words = vec![program];
    words.extend(args);
    words.join(" ")
}

/// Whether a line of `stderr` says `what` in its message: the text after
/// the line's last `": "`, so that a name, which comes before it, cannot
/// say it.
fn says(stderr: &[u8], what: &str) -> bool {
    String::from_utf8_lossy(stderr).lines().any(|line| {
        let message = line.rsplit_once(": ").map_or(line, |(_, message)| message);
        message.contains(what)
    })
}

/// The error a command that did not exit 0 gives, by its message.
fn failure(program: &str, args: &[&str], output: &Output, snapshot: bool) -> ZfsError {
    if says(&output.stderr, "no such pool") {
        return ZfsError::NoPool;
    }
    if snapshot && says(&output.stderr, "dataset does not exist") {
        return ZfsError::NoSnapshot;
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.trim();
    let code = match output.code {
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".into(),
    };
    ZfsError::Other(format!("{}: {code}: {message}", command(program, args)))
}

/// The classes of vdev in `zpool status`, by their headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Data,
    Logs,
    Cache,
    Spares,
    Special,
    Dedup,
}

impl Class {
    fn header(word: &str) -> Option<Class> {
        Some(match word {
            "logs" => Class::Logs,
            "cache" => Class::Cache,
            "spares" => Class::Spares,
            "special" => Class::Special,
            "dedup" => Class::Dedup,
            _ => return None,
        })
    }
}

/// A vdev of `zpool status`, by the name it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) children: Vec<Entry>,
}

/// What `zpool status` shows of a pool's vdevs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    /// Each class's top-level vdevs, in the order shown, the data class
    /// first.
    pub(crate) classes: Vec<(Class, Vec<Entry>)>,
    /// A resilver, rebuild or removal under way, in its line's words.
    pub(crate) busy: Option<String>,
    /// The `remove:` line, under way or done.
    pub(crate) removed: Option<String>,
}

impl Status {
    fn same_shape(&self, other: &Status) -> bool {
        fn same(a: &[Entry], b: &[Entry]) -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(&a.children, &b.children))
        }
        self.classes.len() == other.classes.len()
            && self
                .classes
                .iter()
                .zip(&other.classes)
                .all(|((ca, a), (cb, b))| ca == cb && same(a, b))
    }
}

/// The kind of an interior vdev (or an indirect one) by its name in
/// `zpool status`: `mirror-0`, `raidz2-1`, `draid1:2d:6c:0s-0`,
/// `replacing-0`, `spare-0`, `indirect-0`.
fn interior(name: &str) -> Option<VdevKind> {
    let (stem, index) = name.rsplit_once('-')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match stem {
        "mirror" => VdevKind::Mirror,
        "replacing" => VdevKind::Replacing,
        "spare" => VdevKind::Spare,
        "indirect" => VdevKind::Indirect,
        _ if stem.starts_with("raidz") && stem[5..].bytes().all(|b| b.is_ascii_digit()) => {
            VdevKind::Raidz
        }
        _ if stem.starts_with("draid") => VdevKind::Draid,
        _ => return None,
    })
}

/// `zpool status -P [-g] POOL`'s text: the config's tree and the scan
/// and removal lines ([module docs](self)).
pub(crate) fn parse_status(text: &str, pool: &str) -> Result<Status, String> {
    let mut lines = text.lines();
    let mut named = None;
    let mut busy = None;
    let mut removed = None;
    for line in lines.by_ref() {
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("pool:") {
            named = Some(name.trim().to_string());
        } else if let Some(scan) = trimmed.strip_prefix("scan:") {
            let scan = scan.trim();
            if scan.contains("in progress")
                && (scan.contains("resilver") || scan.contains("rebuild"))
            {
                busy = Some(format!("scan: {scan}"));
            }
        } else if let Some(remove) = trimmed.strip_prefix("remove:") {
            let remove = remove.trim();
            if remove.contains("in progress") {
                busy = Some(format!("remove: {remove}"));
            }
            removed = Some(format!("remove: {remove}"));
        } else if trimmed == "config:" {
            break;
        }
    }
    match named {
        Some(name) if name == pool => {}
        Some(name) => return Err(format!("the status of `{name}`, not `{pool}`")),
        None => return Err("no `pool:` line".into()),
    }
    // A blank line, the column heads, then the tree to a blank line.
    let mut tree = lines
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty());
    match tree.next() {
        Some(heads) if heads.split_whitespace().next() == Some("NAME") => {}
        _ => return Err("no `NAME` heads under `config:`".into()),
    }
    let mut classes: Vec<(Class, Vec<Entry>)> = Vec::new();
    // The path from the class's list to the vdev last read, by depth.
    let mut root_seen = false;
    for line in tree {
        let body = line
            .strip_prefix('\t')
            .ok_or_else(|| format!("a line of the tree that is not indented by a tab: `{line}`"))?;
        let indent = body.len() - body.trim_start_matches(' ').len();
        let name = body
            .split_whitespace()
            .next()
            .ok_or_else(|| format!("an empty line of the tree: `{line}`"))?;
        if indent % 2 != 0 {
            return Err(format!("an odd indent in `{line}`"));
        }
        let depth = indent / 2;
        if depth == 0 {
            if !root_seen {
                if name != pool {
                    return Err(format!("the tree's root is `{name}`, not `{pool}`"));
                }
                root_seen = true;
                classes.push((Class::Data, Vec::new()));
            } else {
                let class =
                    Class::header(name).ok_or_else(|| format!("an unknown class `{name}`"))?;
                classes.push((class, Vec::new()));
            }
            continue;
        }
        let Some((_, top)) = classes.last_mut() else {
            return Err(format!("a vdev before the tree's root: `{line}`"));
        };
        // Walk down to the parent at depth - 1.
        let mut siblings = top;
        for level in 1..depth {
            siblings = &mut siblings
                .last_mut()
                .ok_or_else(|| format!("`{name}` at depth {depth} has no parent at {level}"))?
                .children;
        }
        siblings.push(Entry {
            name: name.to_string(),
            children: Vec::new(),
        });
    }
    if !root_seen {
        return Err("no tree under `config:`".into());
    }
    Ok(Status {
        classes,
        busy,
        removed,
    })
}

/// A GUID as ZFS writes it: unsigned, or signed and taken modulo 2^64.
pub(crate) fn parse_guid(text: &str) -> Option<u64> {
    text.parse::<u64>()
        .ok()
        .or_else(|| text.parse::<i64>().ok().map(|n| n as u64))
}

/// `zpool get -H -p -o property,value ...`'s text.
pub(crate) fn parse_props(text: &str) -> Result<Props, String> {
    let mut values = BTreeMap::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (property, value) = line
            .split_once('\t')
            .ok_or_else(|| format!("a line with no tab: `{line}`"))?;
        if values.insert(property, value).is_some() {
            return Err(format!("`{property}` given twice"));
        }
    }
    let get = |property: &str| {
        values
            .get(property)
            .copied()
            .ok_or_else(|| format!("no `{property}`"))
    };
    let number = |property: &str| {
        let value = get(property)?;
        parse_guid(value).ok_or_else(|| format!("`{property}` is `{value}`, not a number"))
    };
    let switch = |property: &str| match get(property)? {
        "on" => Ok(true),
        "off" => Ok(false),
        value => Err(format!("`{property}` is `{value}`, not on or off")),
    };
    let checkpoint = match get("checkpoint")? {
        "-" | "0" => false,
        value if value.parse::<u64>().is_ok() => true,
        value => return Err(format!("`checkpoint` is `{value}`")),
    };
    Ok(Props {
        guid: number("guid")?,
        load_guid: number("load_guid")?,
        multihost: switch("multihost")?,
        checkpoint,
        readonly: switch("readonly")?,
    })
}

/// The object under `return` in `zfs program -j`'s output, which must
/// be UTF-8.
fn readable(out: &[u8]) -> Result<serde_json::Map<String, Value>, String> {
    let text = std::str::from_utf8(out).map_err(|_| "its output is not UTF-8".to_string())?;
    returned(text)
}

/// The object under `return` in `zfs program -j`'s output.
pub(crate) fn returned(text: &str) -> Result<serde_json::Map<String, Value>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    match value.get("return") {
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(other) => Err(format!("it returned {other}, not a table")),
        None => Err("no `return` in its output".into()),
    }
}

/// A string of a channel program's table.
fn string<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` is not a string"))
}

/// A table of a channel program's table: an object, or an empty list for
/// an empty table.
fn table<'a>(
    map: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match map.get(key) {
        Some(Value::Object(table)) => Ok(Some(table)),
        Some(Value::Array(list)) if list.is_empty() => Ok(None),
        _ => Err(format!("`{key}` is not a table")),
    }
}

/// A local property's value as the snapshot program gives it ([module
/// docs](self)): printable ASCII as a string, any other value as its
/// lower-case hex under `encoding`. Anything the program never gives (a
/// string that is not printable ASCII, hex of the wrong case or of a
/// printable value) is refused.
fn local_value(value: &Value) -> Result<Vec<u8>, String> {
    match value {
        Value::String(text) if manifest::plain(text.as_bytes()) => Ok(text.as_bytes().to_vec()),
        Value::String(_) => Err("is a string that is not printable ASCII".into()),
        Value::Object(map) => {
            if map.len() != 2 || string(map, "encoding")? != "hex" {
                return Err("is not a value in hex".into());
            }
            let hex = string(map, "value")?;
            match manifest::unhex(hex) {
                Some(bytes) if manifest::hex(&bytes) == hex && !manifest::plain(&bytes) => {
                    Ok(bytes)
                }
                _ => Err(format!(
                    "is `{hex}`, not the lower-case hex of a value that is not printable"
                )),
            }
        }
        _ => Err("is not a string or a value in hex".into()),
    }
}

/// Root first, then sorted by name: the order of the manifest's members.
fn member_order(pool: &str, a: &str, b: &str) -> std::cmp::Ordering {
    (a != pool, a).cmp(&(b != pool, b))
}

/// Whether the snapshot program failed after it took some members' `@t`
/// (`taken` above 0), on any errno but `EEXIST`: what it took is
/// destroyed ([module docs](self)). After `EEXIST` a snapshot of that
/// name exists that the program did not take, and `zfs destroy -r`
/// would destroy it too; the program checks every member before it
/// takes any, so this cannot come from the program as written, and is
/// kept for a person (the conservative reading).
fn partly_taken(returned: &serde_json::Map<String, Value>) -> bool {
    let taken = returned
        .get("taken")
        .and_then(Value::as_str)
        .and_then(|n| n.parse::<u64>().ok());
    let errno = returned
        .get("errno")
        .and_then(Value::as_str)
        .and_then(|e| e.parse::<i64>().ok());
    returned.contains_key("failed") && taken.is_some_and(|n| n > 0) && errno != Some(EEXIST)
}

/// The start of an error about what the snapshot program returned.
const PROGRAM_SAYS: &str = "zfs program (snapshot): ";

/// What the snapshot program returned ([module docs](self)).
pub(crate) fn parse_program(
    returned: &serde_json::Map<String, Value>,
    pool: &str,
) -> Result<Program, ZfsError> {
    let other = |why: String| ZfsError::Other(format!("{PROGRAM_SAYS}{why}"));
    if let Some(failed) = returned.get("failed").and_then(Value::as_str) {
        if let Some(errno) = returned.get("errno").and_then(Value::as_str) {
            if errno.parse::<i64>() == Ok(EEXIST) && !returned.contains_key("taken") {
                return Err(ZfsError::Exists);
            }
            let taken = match returned.get("taken").and_then(Value::as_str) {
                Some(n) => format!(", after {n} were taken"),
                None => ", before any was taken".into(),
            };
            return Err(other(format!("{failed}: errno {errno}{taken}")));
        }
        return Err(other(format!("{failed}: failed with no errno")));
    }
    let createtxg = string(returned, "createtxg")
        .and_then(|t| {
            t.parse::<u64>()
                .map_err(|_| format!("`createtxg` is `{t}`"))
        })
        .map_err(other)?;
    let members = table(returned, "members")
        .map_err(other)?
        .ok_or_else(|| other("no members".into()))?;
    let mut out = Vec::with_capacity(members.len());
    for (name, value) in members {
        let Value::Object(member) = value else {
            return Err(other(format!("the member `{name}` is not a table")));
        };
        let guid = |key: &str| {
            let text = string(member, key)?;
            parse_guid(text).ok_or_else(|| format!("`{name}`: `{key}` is `{text}`"))
        };
        let kind = match string(member, "type").map_err(other)? {
            "filesystem" => Kind::Filesystem,
            "volume" => Kind::Volume,
            kind => return Err(other(format!("`{name}` is a {kind}"))),
        };
        let mut local = BTreeMap::new();
        if let Some(set) = table(member, "local").map_err(other)? {
            for (property, value) in set {
                let value = local_value(value)
                    .map_err(|why| other(format!("`{name}`: `{property}` {why}")))?;
                local.insert(property.clone(), value);
            }
        }
        // The agent's own property, should a program let it through.
        local.remove(ASSIST_PROPERTY);
        out.push(manifest::Member {
            name: name.clone(),
            kind,
            guid: guid("guid").map_err(other)?,
            snapshot_guid: guid("snapshot_guid").map_err(other)?,
            local,
        });
    }
    out.sort_by(|a, b| member_order(pool, &a.name, &b.name));
    if out.first().map(|m| m.name.as_str()) != Some(pool) {
        return Err(other(format!("no member is the pool's root `{pool}`")));
    }
    Ok(Program {
        createtxg,
        members: out,
    })
}

/// What the members program returned: `None` where `pool@t` does not
/// exist.
pub(crate) fn parse_members(
    returned: &serde_json::Map<String, Value>,
    pool: &str,
) -> Result<Option<Vec<Member>>, String> {
    if returned.contains_key("absent") {
        return Ok(None);
    }
    let members = table(returned, "members")?.ok_or("no members, and pool@t not absent")?;
    let mut out = Vec::with_capacity(members.len());
    for (name, guid) in members {
        let text = guid
            .as_str()
            .ok_or_else(|| format!("`{name}`'s GUID is not a string"))?;
        out.push(Member {
            name: name.clone(),
            snapshot_guid: parse_guid(text)
                .ok_or_else(|| format!("`{name}`'s GUID is `{text}`"))?,
        });
    }
    out.sort_by(|a, b| member_order(pool, &a.name, &b.name));
    if out.first().map(|m| m.name.as_str()) != Some(pool) {
        return Err(format!(
            "pool@t exists and `{pool}` is not among the members"
        ));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod recordings;
#[cfg(test)]
mod tests;
