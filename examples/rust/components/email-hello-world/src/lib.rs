wit_bindgen::generate!("component");

use crate::exports::examples::email_hello_world::invoke;
use crate::prototype::email;
use crate::wasi::logging::logging;

/// Implmentation of the world specified (`component`) is done on this
/// Component which is exported
struct Component;

const CONFIG_NAME: &str = "hello-world";

const DEFAULT_LOG_CTX: &str = "component-email-hello-world";

impl crate::invoke::Guest for Component {
    fn call() -> String {
        // Retrieve configuration
        let config_token = match email::outgoing_config::get_configuration(CONFIG_NAME) {
            Some(token) => token,
            None => {
                return format!("ERROR: failed to find email configuration [{CONFIG_NAME}]");
            }
        };

        // Build the email to send
        let msg = email::types::OutgoingEmail {
            sender: "admin@example.com".into(),
            recipients: vec!["hello-world@example.com".to_string()],
            bcc: None,
            cc: None,
            subject: "Hello World!".into(),
            html: None,
            text: Some("Textual content is here".into()),
        };

        // Send the email
        match email::outgoing_sender::send_email(&config_token, &msg) {
            Ok(()) => logging::log(
                logging::Level::Info,
                DEFAULT_LOG_CTX,
                "successfully sent email",
            ),
            Err(e) => logging::log(
                logging::Level::Error,
                DEFAULT_LOG_CTX,
                format!("failed to send email: {e}").as_ref(),
            ),
        };
        String::from("Email sent successfully!")
    }
}

export!(Component);
