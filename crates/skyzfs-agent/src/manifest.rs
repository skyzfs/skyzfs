//! The ZFS manifest of a published snapshot (design §5.4; the plan of
//! M2.6, §5): what a restore needs of the pool that object storage does
//! not hold, read in the snapshot's own transaction group by the channel
//! program that takes it. The daemon stores it as it comes, and its row
//! of the snapshot records its length and BLAKE3 hash ([`hash`]).
//!
//! # Version 1
//!
//! Canonical JSON, so that an operator and M3's restore read it without
//! the agent: one object, its keys sorted, no white space, of the form
//!
//! ```text
//! {"createtxg":G,"format":"skyzfs-manifest","members":[M,...],
//!  "pool":"tank","pool_guid":"N","snapshot":"t","version":1}
//! ```
//!
//! where each member M is
//!
//! ```text
//! {"guid":"N","local":{"prop":"value",...},"name":"tank/fs",
//!  "snapshot_guid":"N","type":"filesystem"}
//! ```
//!
//! (the line breaks are this page's). `type` is `filesystem` or
//! `volume`. `local` holds the properties set locally on the member:
//! system properties whose source is `local`, and user properties, as
//! `zfs get -p` gives their values. The members are the pool's root
//! dataset first and then its descendants, sorted by name, each once.
//!
//! The GUIDs are decimal strings, and `createtxg` a number. This
//! module's reading, not the plan's, which names the keys and not their
//! types: a GUID is a random 64-bit number, which a reader of JSON that
//! holds numbers as doubles (JavaScript, `jq` before 1.7) would round; a
//! group number stays below 2^53.
//!
//! A reader takes only these bytes: another format or version, a key
//! missing or more, a value of another type, or the same content written
//! otherwise (white space, another order of keys, another escape) is
//! refused. So the hash the daemon records names one content.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::record;

/// The value of `format`.
pub const FORMAT: &str = "skyzfs-manifest";

/// The version this agent writes and reads.
pub const VERSION: u64 = 1;

/// A manifest ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The pool's name.
    pub pool: String,
    /// The pool's GUID.
    pub pool_guid: u64,
    /// The snapshot's name, `t`.
    pub snapshot: String,
    /// `createtxg` of `pool@t`.
    pub createtxg: u64,
    /// The pool's datasets, the root first, then sorted by name.
    pub members: Vec<Member>,
}

/// A dataset of the pool and its `@t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The dataset's name.
    pub name: String,
    /// A file system or a volume.
    pub kind: Kind,
    /// The dataset's GUID (design §5.4).
    pub guid: u64,
    /// The GUID of its `@t`, by which a restore finds it.
    pub snapshot_guid: u64,
    /// Its properties set locally, by name.
    pub local: BTreeMap<String, String>,
}

/// What a member is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `filesystem`.
    Filesystem,
    /// `volume`.
    Volume,
}

/// Why a manifest is not written or not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Not JSON.
    #[error("not JSON: {0}")]
    Json(String),
    /// Another format.
    #[error("not a manifest of SkyZFS")]
    Format,
    /// Another version.
    #[error("version {0}: this agent reads version {VERSION}")]
    Version(String),
    /// A key missing, a key more, or a value of another type.
    #[error("{0}")]
    Shape(String),
    /// The same content written otherwise.
    #[error("not in canonical form")]
    NotCanonical,
    /// Over the protocol's limit.
    #[error("{0} bytes; at most {max}", max = skyzfs_control::MANIFEST_MAX)]
    TooLarge(usize),
    /// A rule of the members broken.
    #[error("{0}")]
    Rule(String),
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Filesystem => "filesystem",
            Kind::Volume => "volume",
        }
    }
}

impl Manifest {
    /// The manifest's bytes, or why it cannot be one.
    pub fn encode(&self) -> Result<Vec<u8>, ManifestError> {
        self.check()?;
        let mut out = Vec::new();
        out.extend_from_slice(b"{\"createtxg\":");
        out.extend_from_slice(self.createtxg.to_string().as_bytes());
        out.extend_from_slice(b",\"format\":");
        string(&mut out, FORMAT);
        out.extend_from_slice(b",\"members\":[");
        for (i, member) in self.members.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend_from_slice(b"{\"guid\":");
            string(&mut out, &member.guid.to_string());
            out.extend_from_slice(b",\"local\":{");
            for (j, (key, value)) in member.local.iter().enumerate() {
                if j > 0 {
                    out.push(b',');
                }
                string(&mut out, key);
                out.push(b':');
                string(&mut out, value);
            }
            out.extend_from_slice(b"},\"name\":");
            string(&mut out, &member.name);
            out.extend_from_slice(b",\"snapshot_guid\":");
            string(&mut out, &member.snapshot_guid.to_string());
            out.extend_from_slice(b",\"type\":");
            string(&mut out, member.kind.word());
            out.push(b'}');
        }
        out.extend_from_slice(b"],\"pool\":");
        string(&mut out, &self.pool);
        out.extend_from_slice(b",\"pool_guid\":");
        string(&mut out, &self.pool_guid.to_string());
        out.extend_from_slice(b",\"snapshot\":");
        string(&mut out, &self.snapshot);
        out.extend_from_slice(b",\"version\":");
        out.extend_from_slice(VERSION.to_string().as_bytes());
        out.push(b'}');
        if out.len() > skyzfs_control::MANIFEST_MAX {
            return Err(ManifestError::TooLarge(out.len()));
        }
        Ok(out)
    }

    /// The manifest in `bytes`, which must be its canonical form.
    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > skyzfs_control::MANIFEST_MAX {
            return Err(ManifestError::TooLarge(bytes.len()));
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| ManifestError::Json(e.to_string()))?;
        let top = object(
            &value,
            "the manifest",
            &[
                "createtxg",
                "format",
                "members",
                "pool",
                "pool_guid",
                "snapshot",
                "version",
            ],
        )?;
        if top.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err(ManifestError::Format);
        }
        match top.get("version") {
            Some(Value::Number(n)) if n.as_u64() == Some(VERSION) => {}
            Some(other) => return Err(ManifestError::Version(other.to_string())),
            None => return Err(ManifestError::Version("none".into())),
        }
        let Some(Value::Array(members)) = top.get("members") else {
            return Err(ManifestError::Shape("`members` is not a list".into()));
        };
        let manifest = Manifest {
            pool: text(top, "pool")?,
            pool_guid: guid(top, "pool_guid")?,
            snapshot: text(top, "snapshot")?,
            createtxg: top
                .get("createtxg")
                .and_then(Value::as_u64)
                .ok_or_else(|| ManifestError::Shape("`createtxg` is not a number".into()))?,
            members: members.iter().map(member).collect::<Result<_, _>>()?,
        };
        if manifest.encode()? != bytes {
            return Err(ManifestError::NotCanonical);
        }
        Ok(manifest)
    }

    /// Whether the members keep their rules: the pool's root first, then
    /// its descendants sorted by name, each once, and a snapshot name the
    /// agent gives.
    fn check(&self) -> Result<(), ManifestError> {
        let rule = |why: String| Err(ManifestError::Rule(why));
        if !skyzfs_control::valid_name(&self.snapshot) {
            return rule(format!("`{}` is not a name the agent gives", self.snapshot));
        }
        let Some((root, rest)) = self.members.split_first() else {
            return rule("no members".into());
        };
        if root.name != self.pool {
            return rule(format!(
                "the first member is `{}`, not the pool's root `{}`",
                root.name, self.pool
            ));
        }
        let below = format!("{}/", self.pool);
        let mut last: &str = &below;
        for member in rest {
            if !member.name.starts_with(&below) || member.name.len() == below.len() {
                return rule(format!("`{}` is not of the pool", member.name));
            }
            if member.name.as_str() <= last {
                return rule(format!("`{}` is not after `{last}`", member.name));
            }
            last = &member.name;
        }
        Ok(())
    }

    /// Each member's `@t`, as the record keeps them.
    pub fn snapshots(&self) -> Vec<record::Member> {
        self.members
            .iter()
            .map(|m| record::Member {
                name: m.name.clone(),
                snapshot_guid: m.snapshot_guid,
            })
            .collect()
    }
}

/// The BLAKE3 hash of a manifest's bytes, as the daemon's row records it.
pub fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// `text` as a JSON string: `"` and `\` escaped, the control characters
/// as their short escapes or `\u00xx`, the rest as it is.
fn string(out: &mut Vec<u8>, text: &str) {
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// `value` as an object of exactly `keys`.
fn object<'a>(
    value: &'a Value,
    what: &str,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, ManifestError> {
    let Value::Object(map) = value else {
        return Err(ManifestError::Shape(format!("{what} is not an object")));
    };
    if map.len() != keys.len() || !keys.iter().all(|k| map.contains_key(*k)) {
        let mut have: Vec<&str> = map.keys().map(String::as_str).collect();
        have.sort_unstable();
        return Err(ManifestError::Shape(format!(
            "{what} has the keys {have:?}, not {keys:?}"
        )));
    }
    Ok(map)
}

fn text(map: &serde_json::Map<String, Value>, key: &str) -> Result<String, ManifestError> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ManifestError::Shape(format!("`{key}` is not a string")))
}

/// A GUID: a string of decimal digits, as [`u64`] writes it.
fn guid(map: &serde_json::Map<String, Value>, key: &str) -> Result<u64, ManifestError> {
    let text = text(map, key)?;
    match text.parse::<u64>() {
        Ok(n) if n.to_string() == text => Ok(n),
        _ => Err(ManifestError::Shape(format!(
            "`{key}` is not a GUID: `{text}`"
        ))),
    }
}

fn member(value: &Value) -> Result<Member, ManifestError> {
    let map = object(
        value,
        "a member",
        &["guid", "local", "name", "snapshot_guid", "type"],
    )?;
    let Some(Value::Object(local)) = map.get("local") else {
        return Err(ManifestError::Shape("`local` is not an object".into()));
    };
    let local = local
        .iter()
        .map(|(k, v)| match v {
            Value::String(v) => Ok((k.clone(), v.clone())),
            _ => Err(ManifestError::Shape(format!(
                "the local property `{k}` is not a string"
            ))),
        })
        .collect::<Result<_, _>>()?;
    let kind = match map.get("type").and_then(Value::as_str) {
        Some("filesystem") => Kind::Filesystem,
        Some("volume") => Kind::Volume,
        other => {
            return Err(ManifestError::Shape(format!(
                "`type` is {other:?}, not a file system or a volume"
            )))
        }
    };
    Ok(Member {
        name: text(map, "name")?,
        kind,
        guid: guid(map, "guid")?,
        snapshot_guid: guid(map, "snapshot_guid")?,
        local,
    })
}

#[cfg(test)]
mod tests;
