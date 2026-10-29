//! Manifest config for the S3 backend: the keys it forwards to
//! `object_store`, how spellings fold, and where a binding's credentials
//! may come from.

use std::collections::{BTreeMap, HashMap};

use object_store::ClientConfigKey;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};

use crate::plugin::multiplex::BACKEND_CONFIG_KEY;

/// Opts a binding into the host's own AWS identity (`AWS_*` env, then IMDS).
/// Host-owned: see the plugins' `binding_schema`.
pub(crate) const USE_HOST_IDENTITY_KEY: &str = "use_host_identity";

/// Every spelling the host owns: the whole table, since each key decides where
/// a binding connects or as whom, plus the host-identity opt-in. Listed by
/// spelling because the host folds only case and `_`/`-`, so
/// `aws_secret_access_key` is not `secret_access_key` to it.
pub(crate) fn host_owned_spellings() -> impl Iterator<Item = &'static str> {
    CONFIG_KEYS
        .iter()
        .flat_map(|spec| spec.aliases.iter().copied())
        .chain(std::iter::once(USE_HOST_IDENTITY_KEY))
}

/// One manifest config key the s3 backend forwards to `object_store`.
pub(super) struct ConfigKeySpec {
    /// Canonical name, used in `pool_key` and in error messages.
    pub(super) name: &'static str,
    /// Every spelling accepted, already lower-case with `_` (callers normalize first).
    aliases: &'static [&'static str],
    key: AmazonS3ConfigKey,
}

/// Forwarded keys, under every spelling `AmazonS3ConfigKey` accepts. Not here,
/// and refused: `bucket` (the container is the bucket), and the keys that read
/// host files or dial manifest-chosen URLs (`web_identity_token_file`,
/// `container_*`, `metadata_endpoint`, `role_arn`, proxy and certificate keys).
pub(super) const CONFIG_KEYS: &[ConfigKeySpec] = &[
    ConfigKeySpec {
        name: "region",
        aliases: &["region", "aws_region"],
        key: AmazonS3ConfigKey::Region,
    },
    ConfigKeySpec {
        name: "default_region",
        aliases: &["default_region", "aws_default_region"],
        key: AmazonS3ConfigKey::DefaultRegion,
    },
    ConfigKeySpec {
        name: "endpoint",
        aliases: &[
            "endpoint",
            "aws_endpoint",
            "endpoint_url",
            "aws_endpoint_url",
        ],
        key: AmazonS3ConfigKey::Endpoint,
    },
    ConfigKeySpec {
        name: "access_key_id",
        aliases: &["access_key_id", "aws_access_key_id"],
        key: AmazonS3ConfigKey::AccessKeyId,
    },
    ConfigKeySpec {
        name: "secret_access_key",
        aliases: &["secret_access_key", "aws_secret_access_key"],
        key: AmazonS3ConfigKey::SecretAccessKey,
    },
    ConfigKeySpec {
        name: "session_token",
        aliases: &["session_token", "aws_session_token", "aws_token", "token"],
        key: AmazonS3ConfigKey::Token,
    },
    ConfigKeySpec {
        name: "allow_http",
        aliases: &["allow_http", "aws_allow_http"],
        key: AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp),
    },
    ConfigKeySpec {
        name: "virtual_hosted_style_request",
        aliases: &[
            "virtual_hosted_style_request",
            "aws_virtual_hosted_style_request",
        ],
        key: AmazonS3ConfigKey::VirtualHostedStyleRequest,
    },
    ConfigKeySpec {
        name: "skip_signature",
        aliases: &["skip_signature", "aws_skip_signature"],
        key: AmazonS3ConfigKey::SkipSignature,
    },
];

/// Trim, lower-case, `-` → `_`. The host folds `_`/`-`/case for ownership
/// checks (`bindings::canonical_key`) but hands the plugin the written spelling.
pub(super) fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase().replace('-', "_")
}

pub(super) fn lookup(normalized: &str) -> Option<&'static ConfigKeySpec> {
    CONFIG_KEYS.iter().find(|k| k.aliases.contains(&normalized))
}

/// Match `object_store`'s boolean spellings, including values inherited from
/// the host environment. `get_config_value` returns the unparsed spelling.
pub(super) fn virtual_hosted(builder: &AmazonS3Builder) -> anyhow::Result<bool> {
    let value = builder
        .get_config_value(&AmazonS3ConfigKey::VirtualHostedStyleRequest)
        .unwrap_or_else(|| "false".into());
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" | "y" => Ok(true),
        "0" | "false" | "off" | "no" | "n" => Ok(false),
        _ => anyhow::bail!(
            "invalid s3 blobstore config key 'virtual_hosted_style_request'; expected a boolean"
        ),
    }
}

/// One manifest setting after folding its spellings onto a canonical name.
struct Folded<'a> {
    /// The spelling the manifest used, for the conflict message.
    written: &'a str,
    value: &'a str,
    /// The `object_store` key it is written to; `None` for a key this backend
    /// reads itself.
    target: Option<AmazonS3ConfigKey>,
}

/// The bucket-less base builder from a binding's config. Errors name keys,
/// never values (#5470).
pub(super) fn builder_from_config(
    config: &HashMap<String, String>,
) -> anyhow::Result<AmazonS3Builder> {
    // Fold every spelling onto its canonical name first: two spellings of
    // one key with different values would otherwise be applied in `HashMap`
    // order, and the host only folds spellings for keys a schema declares.
    let mut folded: BTreeMap<&'static str, Folded<'_>> = BTreeMap::new();
    for (key, value) in config {
        let normalized = normalize_key(key);
        if normalized == BACKEND_CONFIG_KEY {
            continue;
        }
        let (name, target) = if normalized == USE_HOST_IDENTITY_KEY {
            (USE_HOST_IDENTITY_KEY, None)
        } else {
            let Some(spec) = lookup(&normalized) else {
                anyhow::bail!(
                    "unknown s3 blobstore config key '{key}'; accepted keys: {}",
                    CONFIG_KEYS
                        .iter()
                        .map(|k| k.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            // `build()` prefers `S3Endpoint` (`AWS_ENDPOINT_URL_S3`) over
            // `Endpoint`, so a manifest `endpoint` written to the latter would
            // lose to the host's environment. Write the one that wins.
            let target = if spec.key == AmazonS3ConfigKey::Endpoint {
                AmazonS3ConfigKey::S3Endpoint
            } else {
                spec.key
            };
            (spec.name, Some(target))
        };
        if let Some(first) = folded.get(name) {
            if first.value != value {
                anyhow::bail!(
                    "conflicting values for '{name}' (written '{}' and '{key}', which are one key)",
                    first.written
                );
            }
            continue;
        }
        folded.insert(
            name,
            Folded {
                written: key,
                value,
                target,
            },
        );
    }

    // Whether this binding may use the host's own AWS identity at all.
    let use_host_identity = folded
        .remove(USE_HOST_IDENTITY_KEY)
        .map(|f| f.value.parse::<bool>())
        .transpose()
        .map_err(|_| {
            anyhow::anyhow!("invalid value for '{USE_HOST_IDENTITY_KEY}'; expected true or false")
        })?
        .unwrap_or(false);

    // Every binding states where its credentials come from. Without this, a
    // binding that names none would fall through to the host's environment
    // and instance role: object_store's default chain ends at IMDS.
    let has_access_key = folded.contains_key("access_key_id");
    let has_secret_key = folded.contains_key("secret_access_key");
    if has_access_key != has_secret_key {
        anyhow::bail!(
            "s3 blobstore binding sets one of access_key_id and secret_access_key; static \
             credentials need both"
        );
    }
    let has_static_keys = has_access_key && has_secret_key;
    let anonymous = folded
        .get("skip_signature")
        .is_some_and(|f| f.value == "true");
    let sources = [has_static_keys, anonymous, use_host_identity]
        .into_iter()
        .filter(|set| *set)
        .count();
    if sources != 1 {
        anyhow::bail!(
            "s3 blobstore binding must name exactly one credential source (found {sources}): \
             access_key_id and secret_access_key, or skip_signature: true, or \
             {USE_HOST_IDENTITY_KEY}: true (host-owned under workloadConfig: deny)"
        );
    }

    let mut builder = if use_host_identity {
        AmazonS3Builder::from_env()
    } else {
        AmazonS3Builder::new()
    };
    for setting in folded.into_values() {
        if let Some(target) = setting.target {
            builder = builder.with_config(target, setting.value);
        }
    }
    if virtual_hosted(&builder)?
        && (builder
            .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
            .is_some()
            || builder
                .get_config_value(&AmazonS3ConfigKey::Endpoint)
                .is_some())
    {
        // object_store uses a virtual-hosted endpoint verbatim, ignoring the
        // bucket passed to with_bucket_name. Distinct containers would alias
        // one bucket, and a cross-container move could delete its own copy.
        anyhow::bail!(
            "s3 blobstore cannot combine 'virtual_hosted_style_request' with an 'endpoint'; \
             use path-style requests with an endpoint, or omit the endpoint"
        );
    }
    Ok(builder)
}

#[cfg(test)]
pub(super) mod test_support {
    use std::collections::HashMap;

    pub fn cfg(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// `pairs` plus static keys, for tests about something other than where
    /// credentials come from.
    pub fn keyed(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut config = cfg(&[("access_key_id", "AKIA"), ("secret_access_key", "s")]);
        config.extend(cfg(pairs));
        config
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{cfg, keyed};
    use super::*;

    #[test]
    fn config_keys_match_object_store_spellings() {
        for spec in CONFIG_KEYS {
            assert!(
                spec.aliases.contains(&spec.name),
                "canonical name '{}' must be one of its own spellings",
                spec.name
            );
            for alias in spec.aliases {
                let parsed: AmazonS3ConfigKey = alias
                    .parse()
                    .unwrap_or_else(|e| panic!("object_store rejects '{alias}': {e}"));
                assert_eq!(parsed, spec.key, "'{alias}' resolves to the wrong key");
            }
        }
    }

    #[test]
    fn every_accepted_key_is_host_owned() {
        // Whatever `builder_from_config` accepts, the host must own, so a key
        // added to the table cannot become workload-writable by omission.
        let owned: Vec<&str> = host_owned_spellings().collect();
        for spec in CONFIG_KEYS {
            for alias in spec.aliases {
                assert!(
                    owned.contains(alias),
                    "'{alias}' accepted but not host-owned"
                );
            }
        }
        assert!(owned.contains(&USE_HOST_IDENTITY_KEY));
    }

    #[test]
    fn builder_rejects_unknown_key_naming_it_only() {
        let err = builder_from_config(&cfg(&[("buckets", "VALUE-MUST-NOT-LEAK")]))
            .expect_err("unknown key must be refused")
            .to_string();
        assert!(err.contains("'buckets'"), "names the key: {err}");
        assert!(
            err.contains("accepted keys:"),
            "lists what is accepted: {err}"
        );
        assert!(err.contains("region"), "{err}");
        assert!(
            !err.contains("VALUE-MUST-NOT-LEAK"),
            "must not echo the value: {err}"
        );
    }

    #[test]
    fn builder_rejects_bucket() {
        // The container is the bucket; a fixed one would silently win or lose.
        for spelling in ["bucket", "bucket_name", "aws_bucket", "aws_bucket_name"] {
            assert!(
                builder_from_config(&cfg(&[(spelling, "x")])).is_err(),
                "'{spelling}' must be refused"
            );
        }
    }

    #[test]
    fn builder_rejects_conflicting_spellings_naming_both() {
        // Two spellings of one key with different values would otherwise be
        // applied in `HashMap` order, so the winner would change per bind.
        let err = builder_from_config(&cfg(&[
            ("endpoint", "http://ONE-MUST-NOT-LEAK"),
            ("aws_endpoint", "http://TWO-MUST-NOT-LEAK"),
        ]))
        .expect_err("conflicting spellings must be refused")
        .to_string();
        assert!(err.contains("conflicting values for 'endpoint'"), "{err}");
        assert!(
            err.contains("'endpoint'") && err.contains("'aws_endpoint'"),
            "names both spellings: {err}"
        );
        assert!(
            !err.contains("MUST-NOT-LEAK"),
            "must not echo the values: {err}"
        );
    }

    #[test]
    fn builder_accepts_agreeing_spellings() {
        let builder = builder_from_config(&keyed(&[
            ("endpoint", "http://same:9000"),
            ("aws_endpoint", "http://same:9000"),
        ]))
        .unwrap();
        assert_eq!(
            builder
                .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
                .as_deref(),
            Some("http://same:9000")
        );
    }

    #[test]
    fn builder_accepts_every_spelling() {
        for spec in CONFIG_KEYS {
            // A credential source that does not collide with the key under test.
            let source: &[(&str, &str)] = match spec.name {
                "access_key_id" => &[("secret_access_key", "s")],
                "secret_access_key" => &[("access_key_id", "AKIA")],
                _ => &[("access_key_id", "AKIA"), ("secret_access_key", "s")],
            };
            for alias in spec.aliases {
                let mut config = cfg(source);
                let value = match spec.name {
                    "allow_http" | "virtual_hosted_style_request" | "skip_signature" => "false",
                    _ => "v",
                };
                config.extend(cfg(&[(alias, value)]));
                assert!(
                    builder_from_config(&config).is_ok(),
                    "'{alias}' must be accepted"
                );
            }
        }
    }

    #[test]
    fn builder_requires_a_credential_source() {
        // A binding that names none must not fall through to the host's
        // environment and instance role.
        let err = builder_from_config(&cfg(&[("region", "us-east-1")]))
            .expect_err("no credential source must be refused")
            .to_string();
        assert!(err.contains("credential source"), "{err}");
        assert!(
            err.contains(USE_HOST_IDENTITY_KEY),
            "points at the opt-in: {err}"
        );
    }

    #[test]
    fn builder_accepts_anonymous_access() {
        assert!(builder_from_config(&cfg(&[("skip_signature", "true")])).is_ok());
        // Anything but a literal `true` is not anonymous; object_store rejects
        // the value later, at the probe build.
        assert!(builder_from_config(&cfg(&[("skip_signature", "yes")])).is_err());
    }

    #[test]
    fn builder_uses_host_identity_only_when_asked() {
        assert!(builder_from_config(&cfg(&[(USE_HOST_IDENTITY_KEY, "true")])).is_ok());
        assert!(builder_from_config(&cfg(&[("Use-Host-Identity", "true")])).is_ok());
        let err = builder_from_config(&cfg(&[(USE_HOST_IDENTITY_KEY, "MAYBE-MUST-NOT-LEAK")]))
            .expect_err("a non-bool must be refused")
            .to_string();
        assert!(err.contains(USE_HOST_IDENTITY_KEY), "{err}");
        assert!(
            !err.contains("MUST-NOT-LEAK"),
            "must not echo the value: {err}"
        );
        // Exactly one source: any two of static keys, anonymous and the host
        // identity are refused, naming the count.
        for two in [
            keyed(&[(USE_HOST_IDENTITY_KEY, "true")]),
            keyed(&[("skip_signature", "true")]),
            cfg(&[("skip_signature", "true"), (USE_HOST_IDENTITY_KEY, "true")]),
        ] {
            let err = builder_from_config(&two)
                .expect_err("two credential sources must be refused")
                .to_string();
            assert!(
                err.contains("exactly one credential source (found 2)"),
                "{err}"
            );
        }
        // `skip_signature: false` is not a source, so it may sit next to keys.
        assert!(builder_from_config(&keyed(&[("skip_signature", "false")])).is_ok());
        // Spellings fold like any other key: agreeing ones are one setting,
        // disagreeing ones are refused rather than resolved in map order.
        assert!(
            builder_from_config(&cfg(&[
                (USE_HOST_IDENTITY_KEY, "true"),
                ("Use-Host-Identity", "true"),
            ]))
            .is_ok()
        );
        let err = builder_from_config(&cfg(&[
            (USE_HOST_IDENTITY_KEY, "true"),
            ("use-host-identity", "false"),
        ]))
        .expect_err("disagreeing spellings must be refused")
        .to_string();
        assert!(
            err.contains("conflicting values for 'use_host_identity'"),
            "{err}"
        );
        assert!(
            err.contains("'use_host_identity'") && err.contains("'use-host-identity'"),
            "names both spellings: {err}"
        );
    }

    #[test]
    fn builder_requires_both_static_keys() {
        // object_store refuses a lone key at `build()` too, but the rule is
        // ours to state: half a credential is not a credential source.
        for lone in ["access_key_id", "secret_access_key"] {
            let err = builder_from_config(&cfg(&[(lone, "x")]))
                .expect_err("a lone key must be refused")
                .to_string();
            assert!(
                err.contains("access_key_id") && err.contains("secret_access_key"),
                "'{lone}' alone: {err}"
            );
        }
    }

    #[test]
    fn builder_writes_endpoint_where_build_reads_it() {
        // `S3Endpoint` beats `Endpoint` in `build()`; the manifest must land
        // on the winning side or `AWS_ENDPOINT_URL_S3` in the host env wins.
        let builder = builder_from_config(&keyed(&[("endpoint", "http://manifest:9000")])).unwrap();
        assert_eq!(
            builder
                .get_config_value(&AmazonS3ConfigKey::S3Endpoint)
                .as_deref(),
            Some("http://manifest:9000")
        );
    }
}
