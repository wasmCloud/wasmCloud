wit_bindgen::generate!({ generate_all });

use crate::exports::wasmcloud::wadm::handler::Guest;
use wasmcloud::wadm::types::StatusUpdate;

struct StatusReceiver;

impl Guest for StatusReceiver {
    fn handle_status_update(msg: StatusUpdate) -> Result<(), String> {
        wasi::logging::logging::log(
            wasi::logging::logging::Level::Info,
            "wadm-status",
            &format!(
                "Application '{}' v{} - Status: {:?}",
                msg.app, msg.status.version, msg.status.info.status_type
            ),
        );

        wasi::logging::logging::log(
            wasi::logging::logging::Level::Info,
            "wadm-status",
            &format!("Components found: {}", msg.status.components.len()),
        );

        for component in msg.status.components {
            wasi::logging::logging::log(
                wasi::logging::logging::Level::Info,
                "wadm-status",
                &format!(
                    "Component '{}' - Status: {:?}",
                    component.name, component.info.status_type
                ),
            );
        }

        Ok(())
    }
}

export!(StatusReceiver);
