mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed;
mod nats;

pub use in_memory::InMemoryMessaging;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::{
    BrokerMessage, InMemoryMsgBackend, InMemoryMsgProvider, MsgBackend, MsgId, MsgProvider,
    MultiplexedMessaging, NatsMsgBackend, NatsMsgProvider,
};
pub use nats::NatsMessaging;

/// Returns `true` if the world exports the `wasmcloud:messaging/handler`
/// interface at any version. Matches via [`WitInterface::contains`] rather
/// than set equality, so an exported `handler@0.2.x` is recognized no matter
/// which exact version the component was built against.
pub(crate) fn exports_messaging_handler(world: &crate::wit::WitWorld) -> bool {
    let handler = crate::wit::WitInterface::from("wasmcloud:messaging/handler");
    world.exports.iter().any(|e| e.contains(&handler))
}

/// Parses a comma-separated `subscriptions` config value into trimmed,
/// non-empty subjects. Shared by the in-memory and NATS backends so they
/// agree on how a configured subscription string maps to subjects.
pub(crate) fn parse_subscriptions(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{exports_messaging_handler, parse_subscriptions};
    use crate::wit::{WitInterface, WitWorld};
    use std::collections::HashSet;

    #[test]
    fn recognizes_exported_handler_at_any_version() {
        for export in [
            "wasmcloud:messaging/handler",
            "wasmcloud:messaging/handler@0.2.0",
            "wasmcloud:messaging/handler@0.2.2",
        ] {
            let world = WitWorld {
                imports: HashSet::new(),
                exports: HashSet::from([WitInterface::from(export)]),
            };
            assert!(exports_messaging_handler(&world), "should match {export}");
        }
    }

    #[test]
    fn ignores_non_handler_worlds() {
        // Importing the handler is not exporting it
        let importer = WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.2.0")]),
            exports: HashSet::new(),
        };
        assert!(!exports_messaging_handler(&importer));

        // Exporting other messaging interfaces does not count
        let consumer = WitWorld {
            imports: HashSet::new(),
            exports: HashSet::from([WitInterface::from("wasmcloud:messaging/consumer,types")]),
        };
        assert!(!exports_messaging_handler(&consumer));
    }

    #[test]
    fn parses_single_subject() {
        assert_eq!(
            parse_subscriptions(Some("tasks.task-worker")),
            vec!["tasks.task-worker".to_string()]
        );
    }

    #[test]
    fn parses_multiple_subjects() {
        assert_eq!(
            parse_subscriptions(Some("a,b,c")),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn trims_surrounding_whitespace_and_drops_empties() {
        assert_eq!(
            parse_subscriptions(Some(" tasks.leet , tasks.reverse ,, ")),
            vec!["tasks.leet".to_string(), "tasks.reverse".to_string()]
        );
        assert!(parse_subscriptions(None).is_empty());
    }
}
