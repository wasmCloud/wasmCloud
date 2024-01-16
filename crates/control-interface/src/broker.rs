const DEFAULT_TOPIC_PREFIX: &str = "wasmbus.ctl";

fn prefix(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!(
        "{}.{}",
        topic_prefix
            .as_ref()
            .unwrap_or(&DEFAULT_TOPIC_PREFIX.to_string()),
        lattice
    )
}

pub fn provider_auction_subject(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!("{}.auction.provider", prefix(topic_prefix, lattice))
}

pub fn actor_auction_subject(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!("{}.auction.actor", prefix(topic_prefix, lattice))
}

pub fn advertise_link(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!("{}.linkdefs.put", prefix(topic_prefix, lattice))
}

pub fn remove_link(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!("{}.linkdefs.del", prefix(topic_prefix, lattice))
}

pub fn publish_registries(topic_prefix: &Option<String>, lattice: &str) -> String {
    format!("{}.registries.put", prefix(topic_prefix, lattice))
}

pub fn put_config(
    topic_prefix: &Option<String>,
    lattice: &str,
    entity_id: &str,
    key: &str,
) -> String {
    format!(
        "{}.config.put.{entity_id}.{key}",
        prefix(topic_prefix, lattice)
    )
}

pub fn delete_config(
    topic_prefix: &Option<String>,
    lattice: &str,
    entity_id: &str,
    key: &str,
) -> String {
    format!(
        "{}.config.del.{entity_id}.{key}",
        prefix(topic_prefix, lattice)
    )
}

pub fn clear_config(topic_prefix: &Option<String>, lattice: &str, entity_id: &str) -> String {
    format!("{}.config.del.{entity_id}", prefix(topic_prefix, lattice))
}

pub fn put_label(topic_prefix: &Option<String>, lattice: &str, host_id: &str) -> String {
    format!("{}.labels.{}.put", prefix(topic_prefix, lattice), host_id)
}

pub fn delete_label(topic_prefix: &Option<String>, lattice: &str, host_id: &str) -> String {
    format!("{}.labels.{}.del", prefix(topic_prefix, lattice), host_id)
}

pub mod commands {
    use super::prefix;

    pub fn scale_actor(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.scale", prefix(topic_prefix, lattice), host)
    }

    pub fn stop_actor(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.sa", prefix(topic_prefix, lattice), host) // sa - stop actor
    }

    pub fn start_provider(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.lp", prefix(topic_prefix, lattice), host)
    }

    pub fn stop_provider(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.sp", prefix(topic_prefix, lattice), host)
    }

    pub fn update_actor(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.upd", prefix(topic_prefix, lattice), host)
    }

    pub fn stop_host(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.cmd.{}.stop", prefix(topic_prefix, lattice), host)
    }
}

pub mod queries {
    use super::prefix;

    pub fn link_definitions(topic_prefix: &Option<String>, lattice: &str) -> String {
        format!("{}.get.links", prefix(topic_prefix, lattice))
    }

    pub fn claims(topic_prefix: &Option<String>, lattice: &str) -> String {
        format!("{}.get.claims", prefix(topic_prefix, lattice))
    }

    pub fn host_inventory(topic_prefix: &Option<String>, lattice: &str, host: &str) -> String {
        format!("{}.get.{}.inv", prefix(topic_prefix, lattice), host)
    }

    pub fn hosts(topic_prefix: &Option<String>, lattice: &str) -> String {
        format!("{}.ping.hosts", prefix(topic_prefix, lattice))
    }

    pub fn config(
        topic_prefix: &Option<String>,
        lattice: &str,
        entity_id: &str,
        key: &str,
    ) -> String {
        format!(
            "{}.get.config.{entity_id}.{key}",
            prefix(topic_prefix, lattice),
        )
    }

    pub fn all_config(topic_prefix: &Option<String>, lattice: &str, entity_id: &str) -> String {
        format!("{}.get.config.{entity_id}", prefix(topic_prefix, lattice),)
    }
}
