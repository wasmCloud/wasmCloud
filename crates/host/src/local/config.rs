use core::fmt;

use std::collections::HashMap;
use std::env;

use serde::{de, Deserialize, Deserializer, Serialize};
use url::Url;

/// Declarative local lattice configuration
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lattice {
    /// Actor definitions
    #[serde(default)]
    pub actors: HashMap<String, Actor>,
    /// Link definitions
    #[serde(default)]
    pub links: Vec<Link>,
}

fn deserialize_actor_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: Deserializer<'de>,
{
    struct Visitor;

    impl<'de> de::Visitor<'de> for Visitor {
        type Value = Url;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("relative or absolute actor URL")
        }

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            match s.parse() {
                Ok(url) => Ok(url),
                Err(url::ParseError::RelativeUrlWithoutBase) => {
                    let wd = env::current_dir().map_err(|e| {
                        de::Error::custom(format!("failed to lookup current directory: {e}"))
                    })?;
                    Url::from_file_path(wd.join(s)).map_err(|()| {
                        de::Error::custom(
                            "failed to construct a `file` scheme URL for relative path".to_string(),
                        )
                    })
                }
                Err(e) => Err(de::Error::custom(format!(
                    "failed to parse `{s}` as an actor URL: {e}"
                ))),
            }
        }
    }
    deserializer.deserialize_str(Visitor)
}

/// Actor config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    /// URL of the actor Wasm
    #[serde(deserialize_with = "deserialize_actor_url")]
    pub url: Url,
}

/// TCP socket configuration
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TcpSocket {
    /// Address string to listen on
    pub addr: String, // TODO: Introduce a proper type
}

/// Link config
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Link {
    /// Interface link
    #[serde(rename = "interface")]
    Interface {
        /// Interface name, for example `wasi:logging/logging`
        name: String,
        /// Source actor name
        source: String,
        /// Target actor name
        target: String,
    },
    /// TCP link
    #[serde(rename = "tcp")]
    Tcp {
        /// Socket configuration
        socket: TcpSocket,
        /// Actor chain
        chain: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Context;

    const CONFIG: &str = r#"
[actors.http-parser]
url = "actors/http-parser.wasm"

[actors.http-server]
url = "https://example.com/mypath/http-server.wasm"

[[links]]
kind = "interface"
name = "wasi:http/incoming-handler"
source = "http-parser"
target = "http-server"

[[links]]
kind = "tcp"
chain = [
    "http-parser",
    "http-server",
]
[links.socket]
addr = "[::]:5000"
"#;

    #[test]
    fn parse() -> anyhow::Result<()> {
        let config: Lattice = toml::from_str(CONFIG).context("failed to parse config")?;
        assert_eq!(
            config,
            Lattice {
                actors: HashMap::from([
                    (
                        "http-parser".into(),
                        Actor {
                            url: Url::parse(&format!(
                                "file://{}/actors/http-parser.wasm",
                                env::current_dir()
                                    .expect("failed to lookup current dir")
                                    .display()
                            ))
                            .expect("failed to parse URL"),
                        }
                    ),
                    (
                        "http-server".into(),
                        Actor {
                            url: Url::parse("https://example.com/mypath/http-server.wasm")
                                .expect("failed to parse URL"),
                        }
                    ),
                ]),
                links: vec![
                    Link::Interface {
                        name: "wasi:http/incoming-handler".into(),
                        source: "http-parser".into(),
                        target: "http-server".into(),
                    },
                    Link::Tcp {
                        socket: TcpSocket {
                            addr: "[::]:5000".into(),
                        },
                        chain: vec!["http-parser".into(), "http-server".into(),]
                    },
                ],
            }
        );
        Ok(())
    }
}
