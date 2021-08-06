fn prefix(nsprefix: &Option<String>) -> String {
    format!(
        "wasmbus.ctl.{}",
        nsprefix.as_ref().unwrap_or(&"default".to_string())
    )
}

pub fn control_event(nsprefix: &Option<String>) -> String {
    format!("{}.events", prefix(nsprefix))
}

pub fn provider_auction_subject(nsprefix: &Option<String>) -> String {
    format!("{}.auction.provider", prefix(nsprefix))
}

pub fn actor_auction_subject(nsprefix: &Option<String>) -> String {
    format!("{}.auction.actor", prefix(nsprefix))
}

pub fn advertise_link(ns_prefix: &Option<String>) -> String {
    format!("{}.linkdef.put", prefix(ns_prefix))
}

pub mod commands {
    use super::prefix;

    /// Actor commands require a host target
    pub fn start_actor(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.cmd.{}.la", prefix(nsprefix), host) // la - launch actor
    }

    pub fn stop_actor(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.cmd.{}.sa", prefix(nsprefix), host) // sa - stop actor
    }

    pub fn start_provider(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.cmd.{}.lp", prefix(nsprefix), host)
    }

    pub fn stop_provider(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.cmd.{}.sp", prefix(nsprefix), host)
    }

    pub fn update_actor(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.cmd.{}.upd", prefix(nsprefix), host)
    }
}

pub mod queries {
    use super::prefix;

    pub fn link_definitions(nsprefix: &Option<String>) -> String {
        format!("{}.get.links", prefix(nsprefix))
    }

    pub fn claims(nsprefix: &Option<String>) -> String {
        format!("{}.get.claims", prefix(nsprefix))
    }

    pub fn host_inventory(nsprefix: &Option<String>, host: &str) -> String {
        format!("{}.get.{}.inv", prefix(nsprefix), host)
    }

    pub fn hosts(nsprefix: &Option<String>) -> String {
        format!("{}.get.hosts", prefix(nsprefix))
    }
}
