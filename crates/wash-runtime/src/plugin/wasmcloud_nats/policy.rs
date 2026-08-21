//! Subject, stream, and bucket authorization.
//!
//! Deny-by-default: a workload reaches only what its binding granted. Without
//! this, importing `wasmcloud:nats/core` would reach every subject the
//! connection can see.

use super::config::PolicySpec;

/// Subject prefixes denied regardless of grant.
///
/// `$JS.API` would bypass every stream and consumer check by driving the
/// JetStream API directly; `$KV`/`$OBJ` would bypass bucket checks the same
/// way; `$SYS` is the system account.
const RESERVED: &[&str] = &["$JS.", "$SYS.", "$KV.", "$OBJ."];

/// Compiled per-workload grant.
#[derive(Debug, Clone)]
pub struct PolicyEngine {
    subject_allow: Vec<Pattern>,
    stream_allow: Vec<Pattern>,
    bucket_allow: Vec<Pattern>,
    /// Lattice subjects to deny when the host is lattice-connected.
    lattice_prefixes: Vec<String>,
}

/// A NATS subject pattern: literal tokens, `*` for one token, `>` for the tail.
#[derive(Debug, Clone)]
struct Pattern {
    tokens: Vec<Token>,
    /// True when the pattern ends in `>`.
    trailing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Literal(String),
    Any,
}

impl Pattern {
    fn parse(raw: &str) -> Self {
        let mut tokens = Vec::new();
        let mut trailing = false;
        for part in raw.split('.') {
            match part {
                ">" => {
                    trailing = true;
                    break;
                }
                "*" => tokens.push(Token::Any),
                literal => tokens.push(Token::Literal(literal.to_string())),
            }
        }
        Self { tokens, trailing }
    }

    fn matches(&self, subject: &str) -> bool {
        let parts: Vec<&str> = subject.split('.').collect();
        if self.trailing {
            // `>` matches one or more remaining tokens, never zero.
            if parts.len() <= self.tokens.len() {
                return false;
            }
        } else if parts.len() != self.tokens.len() {
            return false;
        }
        self.tokens
            .iter()
            .zip(parts.iter())
            .all(|(token, part)| match token {
                Token::Any => true,
                Token::Literal(literal) => literal == part,
            })
    }
}

/// Why a subject was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    /// Matched a reserved prefix.
    Reserved,
    /// Not covered by any grant.
    NotGranted,
}

impl PolicyEngine {
    pub fn new(spec: &PolicySpec, lattice_prefixes: Vec<String>) -> Self {
        Self {
            subject_allow: spec
                .subject_allow
                .iter()
                .map(|s| Pattern::parse(s))
                .collect(),
            stream_allow: spec
                .stream_allow
                .iter()
                .map(|s| Pattern::parse(s))
                .collect(),
            bucket_allow: spec
                .bucket_allow
                .iter()
                .map(|s| Pattern::parse(s))
                .collect(),
            lattice_prefixes,
        }
    }

    /// Checks a subject for publish, subscribe, or request.
    pub fn check_subject(&self, subject: &str) -> Result<(), Denied> {
        if RESERVED.iter().any(|prefix| subject.starts_with(prefix))
            || RESERVED.iter().any(|p| subject == p.trim_end_matches('.'))
        {
            return Err(Denied::Reserved);
        }
        if self
            .lattice_prefixes
            .iter()
            .any(|prefix| subject.starts_with(prefix))
        {
            return Err(Denied::Reserved);
        }
        if self.subject_allow.iter().any(|p| p.matches(subject)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }

    /// Checks a stream name for read or management access.
    pub fn check_stream(&self, stream: &str) -> Result<(), Denied> {
        if self.stream_allow.iter().any(|p| p.matches(stream)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }

    /// Checks a KV bucket name.
    pub fn check_bucket(&self, bucket: &str) -> Result<(), Denied> {
        if self.bucket_allow.iter().any(|p| p.matches(bucket)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(subjects: &[&str], streams: &[&str], buckets: &[&str]) -> PolicyEngine {
        PolicyEngine::new(
            &PolicySpec {
                subject_allow: subjects.iter().map(|s| s.to_string()).collect(),
                stream_allow: streams.iter().map(|s| s.to_string()).collect(),
                bucket_allow: buckets.iter().map(|s| s.to_string()).collect(),
            },
            Vec::new(),
        )
    }

    #[test]
    fn empty_grant_denies_everything() {
        let p = engine(&[], &[], &[]);
        assert_eq!(p.check_subject("orders.new"), Err(Denied::NotGranted));
        assert_eq!(p.check_stream("ORDERS"), Err(Denied::NotGranted));
        assert_eq!(p.check_bucket("config"), Err(Denied::NotGranted));
    }

    #[test]
    fn exact_subject_matches() {
        let p = engine(&["orders.new"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert_eq!(p.check_subject("orders.old"), Err(Denied::NotGranted));
    }

    #[test]
    fn star_matches_exactly_one_token() {
        let p = engine(&["orders.*"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert_eq!(p.check_subject("orders"), Err(Denied::NotGranted));
        assert_eq!(p.check_subject("orders.new.eu"), Err(Denied::NotGranted));
    }

    #[test]
    fn star_matches_in_the_middle() {
        let p = engine(&["orders.*.eu"], &[], &[]);
        assert!(p.check_subject("orders.new.eu").is_ok());
        assert_eq!(p.check_subject("orders.new.us"), Err(Denied::NotGranted));
    }

    #[test]
    fn trailing_wildcard_needs_at_least_one_token() {
        let p = engine(&["orders.>"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert!(p.check_subject("orders.new.eu").is_ok());
        assert_eq!(p.check_subject("orders"), Err(Denied::NotGranted));
    }

    #[test]
    fn reserved_prefixes_deny_even_when_granted() {
        let p = engine(&[">"], &[], &[]);
        assert_eq!(
            p.check_subject("$JS.API.STREAM.LIST"),
            Err(Denied::Reserved)
        );
        assert_eq!(
            p.check_subject("$SYS.REQ.SERVER.PING"),
            Err(Denied::Reserved)
        );
        assert_eq!(p.check_subject("$KV.config.key"), Err(Denied::Reserved));
        assert_eq!(p.check_subject("$OBJ.bucket.chunk"), Err(Denied::Reserved));
    }

    #[test]
    fn broad_grant_still_allows_normal_subjects() {
        let p = engine(&[">"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert!(p.check_subject("anything").is_ok());
    }

    #[test]
    fn lattice_prefixes_are_denied() {
        let p = PolicyEngine::new(
            &PolicySpec {
                subject_allow: vec![">".to_string()],
                ..Default::default()
            },
            vec!["wasmbus.".to_string()],
        );
        assert_eq!(p.check_subject("wasmbus.ctl.v1"), Err(Denied::Reserved));
        assert!(p.check_subject("orders.new").is_ok());
    }

    #[test]
    fn streams_and_buckets_use_their_own_grants() {
        let p = engine(&["orders.>"], &["ORDERS"], &["config"]);
        assert!(p.check_stream("ORDERS").is_ok());
        assert_eq!(p.check_stream("SECRETS"), Err(Denied::NotGranted));
        assert!(p.check_bucket("config").is_ok());
        assert_eq!(p.check_bucket("credentials"), Err(Denied::NotGranted));
    }

    #[test]
    fn publish_grant_does_not_imply_stream_access() {
        let p = engine(&["orders.>"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert_eq!(p.check_stream("ORDERS"), Err(Denied::NotGranted));
    }
}
