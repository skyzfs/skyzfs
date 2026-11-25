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
//!   guid,load_guid,multihost,readonly,checkpoint POOL`.
//! - [`Zfs::nocacheflush`]: the module parameter's file,
//!   [`NOCACHEFLUSH`].
//! - [`Zfs::snapshot`]: a channel program, `snapshot.lua` beside this
//!   file, on its standard input: `zfs program -j POOL - POOL T
//!   skyzfs:assist`.
//! - [`Zfs::members`] and [`Zfs::tagged`]: read-only channel programs
//!   (`zfs program -n -j`), `members.lua` and `tagged.lua`.
//! - [`Zfs::held`]: `zfs holds -H pool@t`.
//! - [`Zfs::hold`] and [`Zfs::release`]: `zfs hold -r` and `zfs release
//!   -r`. A release that ZFS refuses for want of the tag is checked with
//!   `zfs holds -H -r pool@t`: no member may keep the tag.
//! - [`Zfs::assist`]: `zfs set skyzfs:assist=V POOL`, then `zpool sync
//!   POOL`.
//!
//! Every command runs in the locale `C`, so that its messages can be
//! read; `no such pool` gives [`ZfsError::NoPool`], `dataset does not
//! exist` [`ZfsError::NoSnapshot`] where a snapshot was named, and
//! anything else [`ZfsError::Other`] with the command and its message.
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
//! fails the program before anything is taken: none is left out of the
//! manifest without a word.
//!
//! The program's values are all strings (`tostring`), and its tables
//! are keyed by name: `zfs program -j` writes a Lua number as a signed
//! 64-bit one (a GUID above 2^63 negative) and a list as an object keyed
//! by position, so names and strings are what reads the same on every
//! version. A GUID is read as unsigned, or as signed and taken modulo
//! 2^64.
//!
//! **This module's readings, to be checked on a real pool (the plan's
//! Q10):**
//!
//! - "Set locally" is a property whose source, as `zfs.get_prop` and the
//!   user-property list give it, is the member itself. A received value
//!   is not local here.
//! - The program's limits are the module's defaults for the most a
//!   program may ask: 100 000 000 instructions and 100 MiB of memory
//!   ([`PROGRAM_INSTRUCTIONS`], [`PROGRAM_MEMORY`]). A pool with more
//!   members than that allows fails the program, which takes nothing.
//! - `zfs program` reads a script named `-` from its standard input.
//! - A checkpoint is none where `zpool get -p` gives `-` or `0`.
//! - A resilver or rebuild is under way where the `scan:` line says
//!   `in progress` and names a resilver or a rebuild, and a removal where
//!   the `remove:` line says `in progress`. A scrub under way is not.
//! - The time limits ([`Limits`]): a read 60 s, a change 120 s.
//!
//! The parsers are tested on output written by hand from OpenZFS's
//! sources and the layouts of the rigs, and on the program's text; the
//! program itself, and the parsers on recorded output of ZFS 2.2 (Ubuntu
//! 24.04) and 2.4 (26.04), wait for a kernel job (Q10).

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
        let output = self
            .host
            .run(program, args, stdin, limit)
            .map_err(|e| ZfsError::Other(format!("{}: {e}", command(program, args))))?;
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
        let out = self.run("zfs", &all, text.as_bytes(), limit, false)?;
        returned(&out).map_err(|why| ZfsError::Other(format!("zfs program: {why}")))
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
        let out = self.zpool(
            &[
                "get",
                "-H",
                "-p",
                "-o",
                "property,value",
                "guid,load_guid,multihost,readonly,checkpoint",
                pool,
            ],
            self.limits.read,
        )?;
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
        let returned = self.program(pool, SNAPSHOT_PROGRAM, &[pool, t, ASSIST_PROPERTY], false)?;
        parse_program(&returned, pool)
    }

    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError> {
        let returned = self.program(pool, MEMBERS_PROGRAM, &[pool, t], true)?;
        parse_members(&returned, pool)
            .map_err(|why| ZfsError::Other(format!("zfs program (members): {why}")))
    }

    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        match self.zfs(&["hold", "-r", tag, &snapshot], self.limits.change, true) {
            Err(ZfsError::Other(why)) if why.contains("tag already exists") => Ok(()),
            done => done.map(drop),
        }
    }

    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError> {
        let snapshot = format!("{pool}@{t}");
        match self.zfs(&["release", "-r", tag, &snapshot], self.limits.change, true) {
            Ok(_) | Err(ZfsError::NoSnapshot) => Ok(()),
            Err(ZfsError::Other(why)) if why.contains("no such tag") => {
                // `-r` releases what has the tag and refuses for what
                // has not: that no member keeps it is checked.
                let holds =
                    match self.zfs(&["holds", "-H", "-r", &snapshot], self.limits.read, true) {
                        Ok(holds) => holds,
                        Err(ZfsError::NoSnapshot) => return Ok(()),
                        Err(e) => return Err(e),
                    };
                let kept: Vec<&str> = holds
                    .lines()
                    .filter_map(|line| {
                        let mut fields = line.split('\t');
                        let name = fields.next()?;
                        (fields.next() == Some(tag)).then_some(name)
                    })
                    .collect();
                if kept.is_empty() {
                    Ok(())
                } else {
                    Err(ZfsError::Other(format!(
                        "zfs release -r {tag} {snapshot}: {} kept the hold: {why}",
                        kept.join(", ")
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }

    fn held(&mut self, pool: &str, t: &str, tag: &str) -> Result<bool, ZfsError> {
        let snapshot = format!("{pool}@{t}");
        match self.zfs(&["holds", "-H", &snapshot], self.limits.read, true) {
            Ok(holds) => Ok(holds.lines().any(|line| {
                let mut fields = line.split('\t');
                fields.next() == Some(snapshot.as_str()) && fields.next() == Some(tag)
            })),
            Err(ZfsError::NoSnapshot) => Ok(false),
            Err(e) => Err(e),
        }
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

/// The error a command that did not exit 0 gives, by its message.
fn failure(program: &str, args: &[&str], output: &Output, snapshot: bool) -> ZfsError {
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.trim();
    if message.contains("no such pool") {
        return ZfsError::NoPool;
    }
    if snapshot && message.contains("dataset does not exist") {
        return ZfsError::NoSnapshot;
    }
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
    Ok(Status { classes, busy })
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

/// Root first, then sorted by name: the order of the manifest's members.
fn member_order(pool: &str, a: &str, b: &str) -> std::cmp::Ordering {
    (a != pool, a).cmp(&(b != pool, b))
}

/// What the snapshot program returned ([module docs](self)).
pub(crate) fn parse_program(
    returned: &serde_json::Map<String, Value>,
    pool: &str,
) -> Result<Program, ZfsError> {
    let other = |why: String| ZfsError::Other(format!("zfs program (snapshot): {why}"));
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
        let property = returned
            .get("property")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let why = returned.get("why").and_then(Value::as_str).unwrap_or("?");
        return Err(other(format!(
            "{failed}: the property `{property}` could not be read ({why}), before any was taken"
        )));
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
                let value = value
                    .as_str()
                    .ok_or_else(|| other(format!("`{name}`: `{property}` is not a string")))?;
                local.insert(property.clone(), value.to_string());
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
mod tests;
