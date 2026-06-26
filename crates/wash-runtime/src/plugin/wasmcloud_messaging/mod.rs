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
    use super::parse_subscriptions;

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
