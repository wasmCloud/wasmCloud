//! The one place `wasmcloud:nats` names its config keys.
//!
//! Two things have to agree about a key: [`super::config::NatsConfig::from_map`],
//! which reads it, and [`crate::plugin::BindingSchema`], which decides whether a
//! workload may write it. Kept as separate lists they agree by convention only,
//! and a key added to the reader and forgotten in the schema silently becomes
//! workload-writable under `workloadConfig: deny` — a grant the operator meant
//! to own, quietly handed back.
//!
//! So the table is the source: the reader looks keys up through [`get`], and the
//! schema is built from the same rows. A key the reader asks for that is not in
//! the table is a bug the tests catch; a key in the table that the reader never
//! asks for is dead weight the same tests name.
//!
//! Aliases live on their row rather than as separate rows, because they are one
//! key: `creds` and `creds-file` are the same setting, and classifying only one
//! of them would both deny nothing and refuse the other spelling as unknown.

use std::collections::HashMap;

use crate::plugin::KeyOwnership;

/// One config key, its aliases, and who it belongs to.
pub struct Key {
    /// The spelling the plugin stores and reports.
    pub canonical: &'static str,
    /// Other spellings the reader accepts for the same setting.
    pub aliases: &'static [&'static str],
    /// Who may write it.
    pub ownership: KeyOwnership,
}

impl Key {
    /// Every spelling of this key, canonical first.
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.canonical).chain(self.aliases.iter().copied())
    }
}

const fn host(canonical: &'static str) -> Key {
    Key {
        canonical,
        aliases: &[],
        ownership: KeyOwnership::Host,
    }
}

const fn host_aliased(canonical: &'static str, aliases: &'static [&'static str]) -> Key {
    Key {
        canonical,
        aliases,
        ownership: KeyOwnership::Host,
    }
}

/// A grant: the operator declares the maximum and a workload may take less.
const fn ceiling(canonical: &'static str) -> Key {
    Key {
        canonical,
        aliases: &[],
        ownership: KeyOwnership::HostCeiling,
    }
}

const fn workload(canonical: &'static str) -> Key {
    Key {
        canonical,
        aliases: &[],
        ownership: KeyOwnership::Workload,
    }
}

/// Every key `wasmcloud:nats` reads.
///
/// Connection keys are [`KeyOwnership::Host`]: an address or a credential has
/// no meaningful "less", and a manifest naming its own servers means a
/// different cluster. Grants are [`KeyOwnership::HostCeiling`], so a workload
/// may ask for a subset of what the operator declared — refusing a workload
/// that asked for *less* would punish least privilege in the right direction.
/// Everything else is the workload's: what it does within the grant.
pub const KEYS: &[Key] = &[
    // Connection — where the binding points and as whom.
    host("servers"),
    host("name"),
    host("jetstream-domain"),
    host("inbox-prefix"),
    host_aliased("creds", &["creds-file"]),
    host("jwt"),
    host_aliased("nkey-seed", &["nkey"]),
    host_aliased("username", &["user"]),
    host("password"),
    host("token"),
    host("tls-ca"),
    host("tls-cert"),
    host("tls-key"),
    host("tls-first"),
    // Grants — what it may reach.
    ceiling("subject-allow"),
    ceiling("stream-allow"),
    ceiling("bucket-allow"),
    // The workload's own behaviour within that grant.
    workload("ack-mode"),
    workload("request-timeout-ms"),
    workload("max-in-flight"),
    workload("subscription-capacity"),
    workload("subscription-capacity-bytes"),
    workload("max-ack-pending"),
    workload("max-deliver"),
    workload("jetstream-subscriptions"),
    workload("core-subscriptions"),
    workload("kv-watches"),
    workload("component"),
];

/// The canonical spelling of `key`, for comparing two spellings of one key.
#[must_use]
pub fn canonical(key: &str) -> String {
    crate::plugin::bindings::canonical_key(key)
}

/// The row `key` belongs to, matched on the canonical spelling or any alias.
///
/// Alias-aware because the reader asks for aliases directly — `parse_auth`
/// reaches for `creds-file` by name — and an alias is the same key, not an
/// unknown one.
pub fn find(key: &str) -> Option<&'static Key> {
    let key = canonical(key);
    KEYS.iter().find(|row| row.spellings().any(|s| s == key))
}

/// Read `canonical` (or any of its aliases) from `cfg`, in any spelling a
/// manifest may have used.
///
/// Trims, and treats an empty value as unset — a key present but blank is a
/// manifest that meant to say nothing, and every caller here wants the default
/// rather than an empty string, or the next alias.
///
/// Aliases have fixed precedence: the canonical spelling first, then each alias
/// in the order [`KEYS`] lists it. Scanning `cfg` once and accepting whichever
/// spelling turned up would let `HashMap` iteration order decide between two a
/// config set both of — and for `creds` vs `creds-file` that is a credential
/// picked at random, differing run to run on one unchanged config.
///
/// # Panics
///
/// Debug builds only, when `canonical` is not in [`KEYS`]: a key the reader
/// asks for and the table does not name would be one the schema cannot
/// classify, which is the drift this module exists to prevent.
pub fn get<'a>(cfg: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    let row = find(key);
    debug_assert!(
        row.is_some(),
        "`{key}` is read but not declared in wasmcloud_nats::keys::KEYS"
    );
    let spellings: Vec<&str> = match row {
        Some(row) => row.spellings().collect(),
        None => vec![key],
    };
    spellings.into_iter().find_map(|spelling| {
        cfg.iter()
            .find(|(k, _)| canonical(k) == spelling)
            .map(|(_, v)| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    })
}

/// The keys the host owns outright, every spelling.
pub fn host_owned() -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .filter(|key| key.ownership == KeyOwnership::Host)
        .flat_map(Key::spellings)
}

/// The grant keys a workload may narrow, every spelling.
pub fn host_ceiling() -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .filter(|key| key.ownership == KeyOwnership::HostCeiling)
        .flat_map(Key::spellings)
}

/// The keys a workload writes freely, every spelling.
pub fn workload_owned() -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .filter(|key| key.ownership == KeyOwnership::Workload)
        .flat_map(Key::spellings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_declared_once_under_one_spelling() {
        let mut seen = std::collections::BTreeSet::new();
        for key in KEYS {
            for spelling in key.spellings() {
                assert_eq!(
                    spelling,
                    crate::plugin::bindings::canonical_key(spelling),
                    "`{spelling}` is not in canonical form, so the schema and the reader would \
                     disagree about it"
                );
                assert!(
                    seen.insert(spelling),
                    "`{spelling}` is declared more than once"
                );
            }
        }
    }

    #[test]
    fn the_schema_classifies_every_spelling() {
        // The alias-completeness rule, enforced rather than documented: the
        // reader accepts `creds` and `creds-file`, so a schema naming only one
        // would deny nothing and refuse the other as unknown.
        let schema = super::super::binding_schema();
        for key in KEYS {
            for spelling in key.spellings() {
                assert_eq!(
                    schema.ownership(spelling),
                    key.ownership,
                    "`{spelling}` is classified differently by the schema than by the table"
                );
            }
        }
    }

    /// Two aliases of one key, both set. Which one wins cannot depend on where
    /// a `HashMap` happened to put them — for `creds` that is a credential.
    #[test]
    fn an_alias_loses_to_the_canonical_spelling_every_time() {
        let cfg: HashMap<String, String> = [
            ("creds".to_string(), "/canonical.creds".to_string()),
            ("creds-file".to_string(), "/alias.creds".to_string()),
        ]
        .into_iter()
        .collect();
        for _ in 0..64 {
            assert_eq!(get(&cfg, "creds"), Some("/canonical.creds"));
            // Asking by the alias is asking for the same key, so it answers the
            // same way.
            assert_eq!(get(&cfg, "creds-file"), Some("/canonical.creds"));
        }

        // A blank canonical is unset rather than empty, so the alias is read.
        let blank: HashMap<String, String> = [
            ("creds".to_string(), "  ".to_string()),
            ("creds-file".to_string(), "/alias.creds".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(get(&blank, "creds"), Some("/alias.creds"));
    }

    #[test]
    fn get_reads_any_spelling_of_an_alias() {
        let cfg: HashMap<String, String> = [("Creds_File".to_string(), " /x.creds ".to_string())]
            .into_iter()
            .collect();
        assert_eq!(get(&cfg, "creds"), Some("/x.creds"));

        let blank: HashMap<String, String> = [("creds".to_string(), "   ".to_string())]
            .into_iter()
            .collect();
        assert_eq!(get(&blank, "creds"), None);
    }
}
