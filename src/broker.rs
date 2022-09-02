const DEFAULT_TOPIC_PREFIX: &str = "wasmbus.ctl";
const EVT_TOPIC_PREFIX: &str = "wasmbus.evt";
const DEFAULT_NS_PREFIX: &str = "default";

fn prefix(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
    format!(
        "{}.{}",
        topic_prefix
            .as_ref()
            .unwrap_or(&DEFAULT_TOPIC_PREFIX.to_string()),
        ns_prefix.as_ref().unwrap_or(&DEFAULT_NS_PREFIX.to_string())
    )
}

pub fn control_event(ns_prefix: &Option<String>) -> String {
    format!(
        "{}.{}",
        EVT_TOPIC_PREFIX,
        ns_prefix.as_ref().unwrap_or(&DEFAULT_NS_PREFIX.to_string())
    )
}

pub fn provider_auction_subject(
    topic_prefix: &Option<String>,
    ns_prefix: &Option<String>,
) -> String {
    format!("{}.auction.provider", prefix(topic_prefix, ns_prefix))
}

pub fn actor_auction_subject(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
    format!("{}.auction.actor", prefix(topic_prefix, ns_prefix))
}

pub fn advertise_link(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
    format!("{}.linkdefs.put", prefix(topic_prefix, ns_prefix))
}

pub fn remove_link(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
    format!("{}.linkdefs.del", prefix(topic_prefix, ns_prefix))
}

pub fn publish_registries(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
    format!("{}.registries.put", prefix(topic_prefix, ns_prefix))
}

pub mod commands {
    use super::prefix;

    /// Actor commands require a host target
    pub fn start_actor(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.la", prefix(topic_prefix, ns_prefix), host) // la - launch actor
    }

    pub fn scale_actor(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.scale", prefix(topic_prefix, ns_prefix), host)
    }

    pub fn stop_actor(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.sa", prefix(topic_prefix, ns_prefix), host) // sa - stop actor
    }

    pub fn start_provider(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.lp", prefix(topic_prefix, ns_prefix), host)
    }

    pub fn stop_provider(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.sp", prefix(topic_prefix, ns_prefix), host)
    }

    pub fn update_actor(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.upd", prefix(topic_prefix, ns_prefix), host)
    }

    pub fn stop_host(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.cmd.{}.stop", prefix(topic_prefix, ns_prefix), host)
    }
}

pub mod queries {
    use super::prefix;

    pub fn link_definitions(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
        format!("{}.get.links", prefix(topic_prefix, ns_prefix))
    }

    pub fn claims(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
        format!("{}.get.claims", prefix(topic_prefix, ns_prefix))
    }

    pub fn host_inventory(
        topic_prefix: &Option<String>,
        ns_prefix: &Option<String>,
        host: &str,
    ) -> String {
        format!("{}.get.{}.inv", prefix(topic_prefix, ns_prefix), host)
    }

    pub fn hosts(topic_prefix: &Option<String>, ns_prefix: &Option<String>) -> String {
        format!("{}.ping.hosts", prefix(topic_prefix, ns_prefix))
    }
}
