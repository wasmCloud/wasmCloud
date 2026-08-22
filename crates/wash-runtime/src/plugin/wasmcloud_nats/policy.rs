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
    /// False when the raw pattern was malformed. A malformed pattern matches
    /// and contains nothing, so a typo cannot widen a grant.
    valid: bool,
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
        let mut valid = !raw.is_empty();
        let parts: Vec<&str> = raw.split('.').collect();
        for (idx, part) in parts.iter().enumerate() {
            match *part {
                // `>` is only the tail. Anything after it made the pattern a
                // typo, not a broader grant.
                ">" => {
                    trailing = true;
                    if idx + 1 != parts.len() {
                        valid = false;
                    }
                    break;
                }
                "*" => tokens.push(Token::Any),
                "" => valid = false,
                literal => tokens.push(Token::Literal(literal.to_string())),
            }
        }
        Self {
            tokens,
            trailing,
            valid,
        }
    }

    fn matches(&self, subject: &str) -> bool {
        if !self.valid {
            return false;
        }
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

    /// True when every subject `other` can match is also matched by `self`.
    ///
    /// This is set containment, not matching: a grant of `orders.*` does not
    /// contain a subscription to `orders.>`, because `>` reaches deeper.
    fn contains(&self, other: &Pattern) -> bool {
        if !self.valid || !other.valid {
            return false;
        }
        // A non-trailing grant cannot cover a trailing request, and lengths
        // must line up exactly when neither side has a tail.
        if other.trailing && !self.trailing {
            return false;
        }
        if self.trailing {
            // `>` needs at least one token beyond the grant's literals, so a
            // request that stops at that length matches subjects the grant
            // never reaches: `*.>` does not contain `a`.
            let minimum = if other.trailing {
                self.tokens.len()
            } else {
                self.tokens.len() + 1
            };
            if other.tokens.len() < minimum {
                return false;
            }
        } else if self.tokens.len() != other.tokens.len() {
            return false;
        }

        self.tokens
            .iter()
            .zip(other.tokens.iter())
            .all(|(grant, request)| match (grant, request) {
                // `*` in the grant covers any single token, wildcard included.
                (Token::Any, _) => true,
                // A literal grant is only satisfied by that same literal; a
                // wildcard request at this position could reach more.
                (Token::Literal(g), Token::Literal(r)) => g == r,
                (Token::Literal(_), Token::Any) => false,
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
    /// A publish or request subject contained `*` or `>`.
    WildcardNotAllowed,
}

/// True when a subscription pattern could expand into a reserved space.
///
/// A leading wildcard makes the whole reserved set reachable, and a wildcard
/// anywhere ahead of a reserved-looking token does the same for that branch.
fn reaches_reserved(pattern: &str, lattice_prefixes: &[String]) -> bool {
    let first = pattern.split('.').next().unwrap_or_default();
    if first == "*" || first == ">" {
        return true;
    }
    RESERVED.iter().any(|p| first == p.trim_end_matches('.'))
        || lattice_prefixes
            .iter()
            .any(|p| first == p.trim_end_matches('.'))
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

    /// True when the subject is reserved or belongs to the host itself.
    fn is_reserved(&self, subject: &str) -> bool {
        RESERVED
            .iter()
            .any(|prefix| subject.starts_with(prefix) || subject == prefix.trim_end_matches('.'))
            || self
                .lattice_prefixes
                .iter()
                .any(|prefix| subject.starts_with(prefix))
    }

    /// Checks a concrete subject for publish or request.
    ///
    /// A published subject must be literal. Matching a request containing `*`
    /// or `>` against the grant patterns would let `orders.>` satisfy a grant
    /// of `orders.*`, and let `wash.*.>` slip past a literal reserved prefix.
    pub fn check_subject(&self, subject: &str) -> Result<(), Denied> {
        if subject.split('.').any(|token| token == "*" || token == ">") {
            return Err(Denied::WildcardNotAllowed);
        }
        if self.is_reserved(subject) {
            return Err(Denied::Reserved);
        }
        if self.subject_allow.iter().any(|p| p.matches(subject)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }

    /// Checks a subscription pattern, which may itself contain wildcards.
    ///
    /// The requested pattern must be *contained* by a grant: every subject the
    /// subscription could receive must be one the grant already allows. A
    /// subject-by-subject match is not enough here, because the request is a
    /// set rather than a point.
    pub fn check_subscription(&self, pattern: &str) -> Result<(), Denied> {
        // A wildcard cannot be allowed to straddle into a reserved space, so
        // reject any pattern whose literal head could reach one.
        if self.is_reserved(pattern) || reaches_reserved(pattern, &self.lattice_prefixes) {
            return Err(Denied::Reserved);
        }
        let requested = Pattern::parse(pattern);
        if self.subject_allow.iter().any(|g| g.contains(&requested)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }

    /// Checks a JetStream consumer's filter subject against the grant.
    ///
    /// Containment only, unlike [`PolicyEngine::check_subscription`]: a filter
    /// selects within a stream the workload was separately granted, so it
    /// cannot straddle into a reserved space the way a raw core subscription
    /// can. Running it through the reserved check would make
    /// `subscriptions: STREAM:>` undeployable under any grant.
    pub fn check_filter(&self, filter: &str) -> Result<(), Denied> {
        let requested = Pattern::parse(filter);
        if self.subject_allow.iter().any(|g| g.contains(&requested)) {
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
    fn publish_subject_may_not_contain_wildcards() {
        // Otherwise `orders.>` would satisfy a grant of `orders.*`.
        let p = engine(&["orders.*"], &[], &[]);
        assert_eq!(p.check_subject("orders.>"), Err(Denied::WildcardNotAllowed));
        assert_eq!(p.check_subject("orders.*"), Err(Denied::WildcardNotAllowed));
        assert!(p.check_subject("orders.new").is_ok());
    }

    #[test]
    fn wildcard_request_cannot_slip_past_reserved() {
        let p = engine(&[">"], &[], &[]);
        assert_eq!(p.check_subject("$JS.*.>"), Err(Denied::WildcardNotAllowed));
        assert_eq!(p.check_subscription("$JS.>"), Err(Denied::Reserved));
        assert_eq!(p.check_subscription("*.API.>"), Err(Denied::Reserved));
        assert_eq!(p.check_subscription(">"), Err(Denied::Reserved));
    }

    #[test]
    fn subscription_must_be_contained_by_the_grant() {
        let p = engine(&["orders.*"], &[], &[]);
        assert!(p.check_subscription("orders.new").is_ok());
        assert!(p.check_subscription("orders.*").is_ok());
        // `>` reaches deeper than the grant allows.
        assert_eq!(p.check_subscription("orders.>"), Err(Denied::NotGranted));
    }

    #[test]
    fn trailing_grant_contains_deeper_subscriptions() {
        let p = engine(&["orders.>"], &[], &[]);
        assert!(p.check_subscription("orders.new").is_ok());
        assert!(p.check_subscription("orders.*").is_ok());
        assert!(p.check_subscription("orders.>").is_ok());
        assert!(p.check_subscription("orders.eu.new").is_ok());
        assert_eq!(p.check_subscription("payments.>"), Err(Denied::NotGranted));
    }

    #[test]
    fn literal_grant_is_not_satisfied_by_a_wildcard_request() {
        let p = engine(&["orders.new"], &[], &[]);
        assert!(p.check_subscription("orders.new").is_ok());
        assert_eq!(p.check_subscription("orders.*"), Err(Denied::NotGranted));
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
    fn trailing_grant_does_not_contain_a_shorter_subscription() {
        // `*.>` matches 2-or-more-token subjects only, so a single-token
        // subscription reaches outside it.
        let p = engine(&["*.>"], &[], &[]);
        assert_eq!(p.check_subject("a"), Err(Denied::NotGranted));
        assert_eq!(p.check_subscription("a"), Err(Denied::NotGranted));
        assert_eq!(p.check_subscription("*"), Err(Denied::Reserved));
        assert!(p.check_subscription("a.b").is_ok());
        assert!(p.check_subscription("a.>").is_ok());
    }

    #[test]
    fn containment_agrees_with_matching_for_literal_subscriptions() {
        // Every literal subscription a grant contains must also be a subject
        // that grant matches.
        let grants = ["orders.>", "*.>", "orders.*", ">", "a.*.c"];
        let subjects = ["a", "a.b", "orders", "orders.new", "orders.new.eu", "a.b.c"];
        for grant in grants {
            let p = engine(&[grant], &[], &[]);
            for subject in subjects {
                assert_eq!(
                    p.check_subscription(subject).is_ok(),
                    p.check_subject(subject).is_ok(),
                    "grant `{grant}` disagrees on `{subject}`"
                );
            }
        }
    }

    #[test]
    fn malformed_grants_do_not_widen() {
        // `>.foo` is a typo, not a grant-all.
        let p = engine(&[">.foo"], &[], &[]);
        assert_eq!(p.check_subject("anything"), Err(Denied::NotGranted));
        assert_eq!(p.check_subscription("anything"), Err(Denied::NotGranted));

        let p = engine(&["orders..new"], &[], &[]);
        assert_eq!(p.check_subject("orders.new"), Err(Denied::NotGranted));
    }

    #[test]
    fn a_filter_may_be_the_full_wildcard_under_a_full_grant() {
        // The stream grant is what scopes a filter, so the reserved-space rule
        // that governs raw subscriptions does not apply to it.
        let p = engine(&[">"], &["ORDERS"], &[]);
        assert!(p.check_filter(">").is_ok());
        assert!(p.check_filter("*.new").is_ok());
        assert_eq!(p.check_subscription(">"), Err(Denied::Reserved));
    }

    #[test]
    fn a_filter_still_has_to_sit_inside_the_subject_grant() {
        let p = engine(&["orders.>"], &["ORDERS"], &[]);
        assert!(p.check_filter("orders.received").is_ok());
        assert!(p.check_filter("orders.>").is_ok());
        assert_eq!(p.check_filter(">"), Err(Denied::NotGranted));
        assert_eq!(p.check_filter("payments.>"), Err(Denied::NotGranted));
    }

    #[test]
    fn publish_grant_does_not_imply_stream_access() {
        let p = engine(&["orders.>"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert_eq!(p.check_stream("ORDERS"), Err(Denied::NotGranted));
    }
}
