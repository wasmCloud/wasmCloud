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
/// way; `$SYS` is the system account. `_nats_push.` is the plugin's own
/// JetStream delivery plane: a workload that could publish there would inject
/// forged deliveries into another workload's handler, and one that could
/// subscribe there would steal them.
const RESERVED_SUBJECT_PREFIXES: &[&str] = &["$JS.", "$SYS.", "$KV.", "$OBJ.", "_nats_push."];

/// The head token of every NATS inbox, shared or per-workload.
///
/// Reserved on the *subscription* path only. Publishing to an inbox is what a
/// responder does with a `reply-to`, so [`PolicyEngine::is_reserved`] must not
/// see this; subscribing to one is reading someone else's replies.
///
/// [`super::conn::workload_inbox_prefix`] builds its per-workload prefixes from
/// this same token so the two cannot drift.
pub const INBOX_TOKEN_PREFIX: &str = "_INBOX";

/// Stream-name prefixes that back a KV or object-store bucket.
///
/// These streams hold the bucket's values as ordinary messages, so reaching one
/// through the stream surface reads the bucket without passing `bucket-allow`.
const BUCKET_BACKING_STREAM_PREFIXES: &[&str] = &["KV_", "OBJ_"];

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
struct NatsSubjectPattern {
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

impl Pattern {
    /// True when some concrete subject is matched by both patterns.
    ///
    /// Containment asks whether one grant swallows another; this asks only
    /// whether they touch, which is the right question for a deny list: a
    /// subscription must be refused if *any* subject it could receive lies in
    /// reserved space, even when most of what it covers is fine.
    fn overlaps(&self, other: &Pattern) -> bool {
        if !self.valid || !other.valid {
            return false;
        }
        // Every position both sides name must admit a common token.
        let shared = self.tokens.len().min(other.tokens.len());
        for (mine, theirs) in self.tokens.iter().zip(other.tokens.iter()).take(shared) {
            match (mine, theirs) {
                (Token::Any, _) | (_, Token::Any) => {}
                (Token::Literal(a), Token::Literal(b)) if a == b => {}
                _ => return false,
            }
        }
        // Past the shorter pattern's last token, only a `>` can keep reaching.
        match self.tokens.len().cmp(&other.tokens.len()) {
            // Same length: both admit a subject of exactly that length, or both
            // demand a longer one. One of each never meets.
            std::cmp::Ordering::Equal => self.trailing == other.trailing,
            std::cmp::Ordering::Less => self.trailing,
            std::cmp::Ordering::Greater => other.trailing,
        }
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
/// Comparing only the head token against a whole prefix is too coarse once a
/// prefix spans more than one token: `runtime.>` and `runtime.*.>` both reach
/// into `runtime.host.` while sharing no head with it. The test is instead
/// whether the requested pattern and the reserved space have any concrete
/// subject in common — see [`Pattern::overlaps`].
fn reaches_reserved(pattern: &str, lattice_prefixes: &[String]) -> bool {
    let first = pattern.split('.').next().unwrap_or_default();
    // A leading wildcard reaches every reserved space at once.
    if first == "*" || first == ">" {
        return true;
    }
    // Inbox space is reserved against subscription, never against publish, so
    // it is tested here rather than in `is_reserved`. The per-workload prefixes
    // join with `_` rather than `.`, so `_INBOX_orders` is one token and a
    // head-token prefix test catches every form.
    if first.starts_with(INBOX_TOKEN_PREFIX) {
        return true;
    }
    let requested = Pattern::parse(pattern);
    RESERVED
        .iter()
        .copied()
        .chain(lattice_prefixes.iter().map(String::as_str))
        .any(|prefix| requested.overlaps(&Pattern::parse(&format!("{prefix}>"))))
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
        // An empty token — a leading, trailing, or doubled `.` — is not a
        // subject any grant can cover, so refusing it here is the same answer
        // the grant walk would give, just without the round-trip to a server
        // that would answer with a protocol error.
        if subject.is_empty() || subject.split('.').any(str::is_empty) {
            return Err(Denied::NotGranted);
        }
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

    /// Checks the subject a stored message was published on.
    ///
    /// Pattern-match only, and for the same reason
    /// [`PolicyEngine::check_filter`] skips it: the message came out of a
    /// stream the workload was separately granted, so its subject is already
    /// stream-scoped and cannot straddle into a reserved space the way a raw
    /// subscription can. What this does add is the subject boundary the
    /// declarative path enforces on consumer filters — without it a narrow
    /// `subject-allow` beside a wide `stream-allow` would bound what a
    /// workload may subscribe to but not what it may read back directly.
    pub fn check_stored_subject(&self, subject: &str) -> Result<(), Denied> {
        if self.subject_allow.iter().any(|p| p.matches(subject)) {
            Ok(())
        } else {
            Err(Denied::NotGranted)
        }
    }

    /// Checks a stream name for read or management access.
    ///
    /// A bucket's backing stream is reachable here only when the bucket itself
    /// was granted: `KV_secrets` holds every value in the `secrets` bucket as
    /// ordinary messages, so a `stream-allow: >` that reached it would read the
    /// bucket straight past `bucket-allow`.
    pub fn check_stream(&self, stream: &str) -> Result<(), Denied> {
        // `$`-headed names are the server's own internal streams.
        if stream.starts_with('$') {
            return Err(Denied::Reserved);
        }
        if let Some(bucket) = BUCKET_BACKING_STREAM_PREFIXES
            .iter()
            .find_map(|prefix| stream.strip_prefix(prefix))
        {
            return if self.bucket_allow.iter().any(|p| p.matches(bucket)) {
                Ok(())
            } else {
                Err(Denied::Reserved)
            };
        }
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

    /// The lattice prefix spans two tokens, so a subscription that stops
    /// short of it — or wildcards its way through — still lands inside it.
    #[test]
    fn a_shorter_wildcard_head_cannot_straddle_a_multi_token_prefix() {
        let p = PolicyEngine::new(
            &PolicySpec {
                subject_allow: vec![">".to_string()],
                ..Default::default()
            },
            vec!["runtime.host.".to_string()],
        );
        assert_eq!(p.check_subscription("runtime.>"), Err(Denied::Reserved));
        assert_eq!(p.check_subscription("runtime.*.>"), Err(Denied::Reserved));
        assert_eq!(
            p.check_subscription("runtime.host.>"),
            Err(Denied::Reserved)
        );
        // A sibling that shares the head but never reaches the prefix is fine.
        assert!(p.check_subscription("runtime.app.>").is_ok());
    }

    /// Replies have to be publishable — that is what a responder does with a
    /// `reply-to` — but nobody gets to listen to somebody else's.
    #[test]
    fn inbox_space_is_reserved_against_subscription_but_not_publish() {
        let p = engine(&[">"], &[], &[]);
        assert_eq!(p.check_subscription("_INBOX.>"), Err(Denied::Reserved));
        assert_eq!(
            p.check_subscription("_INBOX_payments.>"),
            Err(Denied::Reserved)
        );
        assert_eq!(
            p.check_subscription("_INBOX_orders.reply"),
            Err(Denied::Reserved)
        );
        assert!(p.check_subject("_INBOX.abc123").is_ok());
        assert!(p.check_subject("_INBOX_payments.abc").is_ok());
    }

    /// The plugin's own push-delivery plane: forging a delivery into it, or
    /// stealing one out of it, is denied however broad the grant.
    #[test]
    fn the_push_delivery_plane_is_reserved() {
        let p = engine(&[">"], &[], &[]);
        assert_eq!(
            p.check_subject("_nats_push.ORDERS.workers"),
            Err(Denied::Reserved)
        );
        assert_eq!(p.check_subscription("_nats_push.>"), Err(Denied::Reserved));
    }

    /// A subject with an empty token is not a subject at all.
    #[test]
    fn empty_tokens_are_refused_before_the_server_sees_them() {
        let p = engine(&[">"], &[], &[]);
        assert_eq!(p.check_subject(""), Err(Denied::NotGranted));
        assert_eq!(p.check_subject(".foo"), Err(Denied::NotGranted));
        assert_eq!(p.check_subject("a..b"), Err(Denied::NotGranted));
        assert_eq!(
            engine(&["orders.*"], &[], &[]).check_subject("orders."),
            Err(Denied::NotGranted)
        );
        assert!(p.check_subject("orders.new").is_ok());
    }

    /// A bucket's backing stream reads the bucket, so it answers to
    /// `bucket-allow` rather than to `stream-allow`.
    #[test]
    fn bucket_backing_streams_are_routed_through_bucket_allow() {
        let wide = engine(&[], &[">"], &[]);
        assert_eq!(wide.check_stream("KV_secrets"), Err(Denied::Reserved));
        assert_eq!(wide.check_stream("OBJ_media"), Err(Denied::Reserved));
        assert!(wide.check_stream("ORDERS").is_ok());

        let granted = engine(&[], &[">"], &["config"]);
        assert!(granted.check_stream("KV_config").is_ok());
        assert_eq!(granted.check_stream("KV_secrets"), Err(Denied::Reserved));
        assert_eq!(granted.check_stream("OBJ_media"), Err(Denied::Reserved));

        // The server's own internal streams are never reachable.
        assert_eq!(wide.check_stream("$MQTT_msgs"), Err(Denied::Reserved));
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
    fn a_stored_subject_is_matched_not_contained() {
        // A stored message carries one literal subject, so the point-match
        // rule applies rather than the containment rule filters go through.
        let p = engine(&["orders.eu.>"], &["ORDERS"], &[]);
        assert!(p.check_stored_subject("orders.eu.new").is_ok());
        assert_eq!(
            p.check_stored_subject("orders.us.new"),
            Err(Denied::NotGranted)
        );
        assert_eq!(
            p.check_stored_subject("orders.internal.audit"),
            Err(Denied::NotGranted)
        );
    }

    #[test]
    fn a_stored_subject_may_sit_in_reserved_space() {
        // A bucket's own `$KV.` messages are reachable only through a bucket
        // grant, so running them through the reserved walk would make a
        // granted bucket unreadable.
        let p = engine(&[">"], &[], &["config"]);
        assert!(p.check_stored_subject("$KV.config.key").is_ok());
        assert_eq!(p.check_subject("$KV.config.key"), Err(Denied::Reserved));
    }

    #[test]
    fn publish_grant_does_not_imply_stream_access() {
        let p = engine(&["orders.>"], &[], &[]);
        assert!(p.check_subject("orders.new").is_ok());
        assert_eq!(p.check_stream("ORDERS"), Err(Denied::NotGranted));
    }
}
