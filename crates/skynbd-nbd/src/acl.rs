//! Per-identity export ACLs (auth design §3.3).
//!
//! With mTLS on (design §3.1), every connection carries a verified
//! identity — the client cert's URI SAN (SPIFFE ID), DNS SAN, or CN.
//! Rules glob-match that identity to a set of export globs plus a write
//! bit. No matching rule → the export "does not exist" for that client
//! (`REP_ERR_UNKNOWN` — no existence oracle); `write = false` forces the
//! export read-only for the connection.

/// One ACL rule: identity glob → export globs + write permission.
#[derive(Debug, Clone)]
pub struct AclRule {
    pub identity: String,
    pub exports: Vec<String>,
    pub write: bool,
}

/// A connection's resolved access: its verified identity against the
/// rule table. Rules are evaluated per export at option time.
#[derive(Debug, Clone)]
pub struct ConnAccess {
    pub identity: String,
    rules: std::sync::Arc<Vec<AclRule>>,
}

impl ConnAccess {
    pub fn new(identity: String, rules: std::sync::Arc<Vec<AclRule>>) -> Self {
        Self { identity, rules }
    }

    /// `None` = denied (indistinguishable from nonexistent); `Some(w)` =
    /// allowed, writable iff `w`. Multiple matches OR their write bits.
    pub fn access(&self, export: &str) -> Option<bool> {
        let mut write = None;
        for r in self.rules.iter() {
            if glob_match(&r.identity, &self.identity)
                && r.exports.iter().any(|e| glob_match(e, export))
            {
                write = Some(write.unwrap_or(false) || r.write);
            }
        }
        write
    }
}

/// `*`-wildcard glob (no other metacharacters).
pub fn glob_match(pat: &str, s: &str) -> bool {
    fn inner(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], s) || (!s.is_empty() && inner(p, &s[1..])),
            (Some(pc), Some(sc)) if pc == sc => inner(&p[1..], &s[1..]),
            _ => false,
        }
    }
    inner(pat.as_bytes(), s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn access(rules: Vec<AclRule>, id: &str) -> ConnAccess {
        ConnAccess::new(id.to_string(), Arc::new(rules))
    }

    #[test]
    fn deny_by_default() {
        let a = access(vec![], "spiffe://prod/agent/a1");
        assert_eq!(a.access("vol1/head"), None);
    }

    #[test]
    fn identity_and_export_globs() {
        let rules = vec![AclRule {
            identity: "spiffe://prod/agent/*".into(),
            exports: vec!["*/head".into(), "*/snap/*".into()],
            write: true,
        }];
        let a = access(rules.clone(), "spiffe://prod/agent/a1");
        assert_eq!(a.access("vol1/head"), Some(true));
        assert_eq!(a.access("vol1/snap/s1"), Some(true));
        assert_eq!(a.access("vol1/rpo"), None);
        let other = access(rules, "spiffe://prod/restore/r1");
        assert_eq!(other.access("vol1/head"), None);
    }

    #[test]
    fn write_bits_or_across_rules() {
        let rules = vec![
            AclRule {
                identity: "spiffe://prod/*".into(),
                exports: vec!["*".into()],
                write: false,
            },
            AclRule {
                identity: "spiffe://prod/agent/*".into(),
                exports: vec!["db/head".into()],
                write: true,
            },
        ];
        let a = access(rules, "spiffe://prod/agent/a1");
        assert_eq!(a.access("db/head"), Some(true)); // ro rule + rw rule
        assert_eq!(a.access("cache/head"), Some(false)); // ro only
    }

    #[test]
    fn glob_edge_cases() {
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "x"));
        assert!(!glob_match("", "x"));
        assert!(glob_match("a*c", "abc"));
        assert!(glob_match("a*c", "ac"));
        assert!(!glob_match("a*c", "ab"));
    }
}
