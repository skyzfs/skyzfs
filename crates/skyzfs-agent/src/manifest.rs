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
//! {"guid":"N","local":{"prop":V,...},"name":"tank/fs",
//!  "snapshot_guid":"N","type":"filesystem"}
//! ```
//!
//! (the line breaks are this page's). `type` is `filesystem` or
//! `volume`. `local` holds the properties set locally on the member:
//! system properties whose source is `local`, and user properties, as
//! `zfs get -p` gives their values. The members are the pool's root
//! dataset first and then its descendants, sorted by name, each once.
//!
//! A value V is bytes, which ZFS does not require to be UTF-8. One whose
//! bytes are all printable ASCII (0x20 to 0x7e) is written as a JSON
//! string; any other as
//! `{"encoding":"hex","value":"6361c3a9"}`, its bytes in lower-case hex.
//! So a reader never guesses what a value's bytes were, and the same
//! value has one form. (Amended in place by M2.6c-2, the review of #147,
//! before any manifest was published: a value was a string before.)
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
    /// Its properties set locally, by name: each value's bytes ([module
    /// docs](self)).
    pub local: BTreeMap<String, Vec<u8>>,
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
                local_value(&mut out, value);
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

/// Whether `value` is written as a JSON string: printable ASCII only.
pub fn plain(value: &[u8]) -> bool {
    value.iter().all(|b| (0x20..=0x7e).contains(b))
}

/// The value of a local property ([module docs](self)).
fn local_value(out: &mut Vec<u8>, value: &[u8]) {
    match std::str::from_utf8(value) {
        Ok(text) if plain(value) => string(out, text),
        _ => {
            out.extend_from_slice(b"{\"encoding\":\"hex\",\"value\":\"");
            out.extend_from_slice(hex(value).as_bytes());
            out.extend_from_slice(b"\"}");
        }
    }
}

/// `bytes` in lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes of lower-case or upper-case hex `text`; `None` where it is
/// not hex (a sign, which `from_str_radix` takes, included).
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

/// A local property's value as written ([module docs](self)): a string,
/// or the object of its hex.
fn read_local(key: &str, value: &Value) -> Result<Vec<u8>, ManifestError> {
    let shape = |why: &str| ManifestError::Shape(format!("the local property `{key}` {why}"));
    match value {
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        Value::Object(_) => {
            let map = object(value, "an encoded value", &["encoding", "value"])?;
            if map.get("encoding").and_then(Value::as_str) != Some("hex") {
                return Err(shape("is not in an encoding this agent reads"));
            }
            let text = map
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| shape("has no string `value`"))?;
            unhex(text).ok_or_else(|| shape("is not hex"))
        }
        _ => Err(shape("is not a string or an encoded value")),
    }
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
        .map(|(k, v)| Ok((k.clone(), read_local(k, v)?)))
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
