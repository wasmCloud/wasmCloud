use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use clap::Subcommand;
use nkeys::{KeyPair, KeyPairType};
use serde_json::json;
use crate::lib::cli::CommandOutput;
use crate::lib::config::WASH_DIRECTORIES;
use crate::lib::keys::{fs::KeyDir, KeyManager};

const NKEYS_EXTENSION: &str = ".nk";

#[derive(Debug, Clone, Subcommand)]
#[allow(clippy::enum_variant_names)]
pub enum KeysCliCommand {
    #[clap(name = "gen", about = "Generates a keypair")]
    GenCommand {
        /// The type of keypair to generate. May be Account, User, Module (or Component), Service (or Provider), Server (or Host), Operator, Cluster, Curve (xkey)
        keytype: String,
    },
    #[clap(name = "get", about = "Retrieves a keypair and prints the contents")]
    GetCommand {
        #[clap(help = "The name of the key to output")]
        keyname: String,
        #[clap(
            short = 'd',
            long = "directory",
            env = "WASH_KEYS",
            hide_env_values = true,
            help = "Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`"
        )]
        directory: Option<PathBuf>,
    },
    #[clap(name = "list", about = "Lists all keypairs in a directory")]
    ListCommand {
        #[clap(
            short = 'd',
            long = "directory",
            env = "WASH_KEYS",
            hide_env_values = true,
            help = "Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`"
        )]
        directory: Option<PathBuf>,
    },
}

pub fn handle_command(command: KeysCliCommand) -> Result<CommandOutput> {
    match command {
        KeysCliCommand::GenCommand { keytype } => {
            let kt = keytype_parser(&keytype)?;
            generate(&kt)
        }
        KeysCliCommand::GetCommand { keyname, directory } => get(&keyname, directory),
        KeysCliCommand::ListCommand { directory } => list(directory),
    }
}

pub fn keytype_parser(keytype: &str) -> Result<KeyPairType> {
    match keytype.to_lowercase().as_str() {
        "account" => Ok(KeyPairType::Account),
        "user" => Ok(KeyPairType::User),
        "module" | "component" => Ok(KeyPairType::Module),
        "service" | "provider" => Ok(KeyPairType::Service),
        "server" | "host" => Ok(KeyPairType::Server),
        "operator" => Ok(KeyPairType::Operator),
        "cluster" => Ok(KeyPairType::Cluster),
        "x25519" | "curve" => Ok(KeyPairType::Curve),
        _ => Err(anyhow::anyhow!(
            "Invalid key type. Must be one of Account, User, Module (or Component), Service (or Provider), Server (or Host), Operator, Cluster, Curve (xkey)"
        )),
    }
}
/// Generates a keypair of the specified `KeyPairType`
pub fn generate(kt: &KeyPairType) -> Result<CommandOutput> {
    let kp = KeyPair::new(kt.clone());
    let seed = kp.seed()?;

    let mut map = HashMap::new();
    map.insert("public_key".to_string(), json!(kp.public_key()));
    map.insert("seed".to_string(), json!(seed));
    Ok(CommandOutput::new(
        format!(
            "Public Key: {}\nSeed: {}\n\nRemember that the seed is private, treat it as a secret.",
            kp.public_key(),
            seed,
        ),
        map,
    ))
}

/// Retrieves a keypair by name in a specified directory, or $`WASH_KEYS` ($HOME/.wash/keys) if directory is not specified
pub fn get(keyname: &str, directory: Option<PathBuf>) -> Result<CommandOutput> {
    let key_dir = KeyDir::new(determine_directory(directory)?)?;
    // Trim off the ".nk" for backwards compat
    let key = key_dir
        .get(keyname.trim_end_matches(NKEYS_EXTENSION))?
        .ok_or_else(|| anyhow::anyhow!("Key {} doesn't exist", keyname))?;

    Ok(CommandOutput::from_key_and_text("seed", key.seed()?))
}

/// Lists all keypairs (file extension .nk) in a specified directory or $`WASH_KEYS($HOME/.wash/keys)` if directory is not specified
pub fn list(directory: Option<PathBuf>) -> Result<CommandOutput> {
    let key_dir = KeyDir::new(determine_directory(directory)?)?;

    let keys = key_dir.list_names()?;

    let mut map = HashMap::new();
    map.insert("keys".to_string(), json!(keys));
    Ok(CommandOutput::new(
        format!(
            "====== Keys found in {} ======\n{}",
            key_dir.display(),
            keys.join("\n")
        ),
        map,
    ))
}

fn determine_directory(directory: Option<PathBuf>) -> Result<PathBuf> {
    directory.ok_or("no directory").or(WASH_DIRECTORIES.create_keys_dir())
}

#[cfg(test)]
mod tests {

    use super::{generate, keytype_parser, KeysCliCommand};
    use clap::Parser;
    use nkeys::KeyPairType;
    use serde::Deserialize;
    use std::path::PathBuf;

    #[derive(Debug, Parser)]
    struct Cmd {
        #[clap(subcommand)]
        keys: KeysCliCommand,
    }
    #[test]
    fn test_generate_basic_test() {
        let kt = KeyPairType::Account;

        let keypair = generate(&kt).unwrap();

        assert!(keypair.text.contains("Public Key: "));
        assert!(keypair.text.contains("Seed: "));
        assert!(keypair
            .text
            .contains("Remember that the seed is private, treat it as a secret."));
        assert_ne!(keypair.text, "");
        assert!(!keypair.map.is_empty());
    }

    #[derive(Debug, Clone, Deserialize)]
    struct KeyPairJson {
        public_key: String,
        seed: String,
    }

    #[test]
    fn test_generate_valid_keypair() {
        let sample_public_key = "MBBLAHS7MCGNQ6IR4ZDSGRIAF7NVS7FCKFTKGO5JJJKN2QQRVAH7BSIO";
        let sample_seed = "SMAH45IUULL57OSX23NOOOTLSVNQOORMDLE3Y3PQLJ4J5MY7MN2K7BIFI4";

        let kt = KeyPairType::Module;

        let keypair_json = generate(&kt).unwrap();
        let keypair: KeyPairJson =
            serde_json::from_str(serde_json::to_string(&keypair_json.map).unwrap().as_str())
                .unwrap();

        assert_eq!(keypair.public_key.len(), sample_public_key.len());
        assert_eq!(keypair.seed.len(), sample_seed.len());
        assert!(keypair.public_key.starts_with('M'));
        assert!(keypair.seed.starts_with("SM"));
    }

    #[test]
    fn test_generate_all_types() {
        let sample_public_key = "MBBLAHS7MCGNQ6IR4ZDSGRIAF7NVS7FCKFTKGO5JJJKN2QQRVAH7BSIO";
        let sample_seed = "SMAH45IUULL57OSXNOOAKOTLSVNQOORMDLE3Y3PQLJ4J5MY7MN2K7BIFI4";

        let account_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Account).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let user_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::User).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let module_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Module).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let service_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Service).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let server_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Server).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let operator_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Operator).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        let cluster_keypair: KeyPairJson = serde_json::from_str(
            serde_json::to_string(&generate(&KeyPairType::Cluster).unwrap().map)
                .unwrap()
                .as_str(),
        )
        .unwrap();

        assert!(account_keypair.public_key.starts_with('A'));
        assert_eq!(account_keypair.public_key.len(), sample_public_key.len());
        assert!(account_keypair.seed.starts_with("SA"));
        assert_eq!(account_keypair.seed.len(), sample_seed.len());

        assert!(user_keypair.public_key.starts_with('U'));
        assert_eq!(user_keypair.public_key.len(), sample_public_key.len());
        assert!(user_keypair.seed.starts_with("SU"));
        assert_eq!(user_keypair.seed.len(), sample_seed.len());

        assert!(module_keypair.public_key.starts_with('M'));
        assert_eq!(module_keypair.public_key.len(), sample_public_key.len());
        assert!(module_keypair.seed.starts_with("SM"));
        assert_eq!(module_keypair.seed.len(), sample_seed.len());

        assert!(service_keypair.public_key.starts_with('V'));
        assert_eq!(service_keypair.public_key.len(), sample_public_key.len());
        assert!(service_keypair.seed.starts_with("SV"));
        assert_eq!(service_keypair.seed.len(), sample_seed.len());

        assert!(server_keypair.public_key.starts_with('N'));
        assert_eq!(server_keypair.public_key.len(), sample_public_key.len());
        assert!(server_keypair.seed.starts_with("SN"));
        assert_eq!(server_keypair.seed.len(), sample_seed.len());

        assert!(operator_keypair.public_key.starts_with('O'));
        assert_eq!(operator_keypair.public_key.len(), sample_public_key.len());
        assert!(operator_keypair.seed.starts_with("SO"));
        assert_eq!(operator_keypair.seed.len(), sample_seed.len());

        assert!(cluster_keypair.public_key.starts_with('C'));
        assert_eq!(cluster_keypair.public_key.len(), sample_public_key.len());
        assert!(cluster_keypair.seed.starts_with("SC"));
        assert_eq!(cluster_keypair.seed.len(), sample_seed.len());
    }

    #[test]
    /// Enumerates multiple options of the `gen` command to ensure API doesn't
    /// change between versions. This test will fail if `wash keys gen <type>`
    /// changes syntax, ordering of required elements, or flags.
    fn test_gen_comprehensive() {
        let key_gen_types = [
            "acCount",
            "usEr",
            "module",
            "COMPONENT",
            "SERVICE",
            "provider",
            "server",
            "HOST",
            "operator",
            "CLUSTER",
        ];

        key_gen_types
            .iter()
            .map(|cmd| cmd.to_lowercase())
            .for_each(|cmd| {
                let gen_cmd: Cmd = clap::Parser::try_parse_from(["keys", "gen", &cmd]).unwrap();
                match gen_cmd.keys {
                    KeysCliCommand::GenCommand { keytype } => {
                        use KeyPairType::*;
                        let parsed_keytype = keytype_parser(&keytype).unwrap();
                        match parsed_keytype {
                            Account => assert_eq!(&cmd, "account"),
                            User => assert_eq!(&cmd, "user"),
                            Module => assert!(cmd.eq("module") || cmd.eq("component")),
                            Service => assert!(cmd.eq("service") || cmd.eq("provider")),
                            Server => assert!(cmd.eq("server") || cmd.eq("host")),
                            Operator => assert_eq!(&cmd, "operator"),
                            Cluster => assert_eq!(&cmd, "cluster"),
                            Curve => assert_eq!(&cmd, "curve"),
                        }
                    }
                    _ => panic!("`keys gen` constructed incorrect command"),
                };
            });
    }

    #[test]
    fn test_invalid_keytype_input() {
        let key_gen_types = [
            "accout", "USE", "moDUl", "actors", "SEVICE", "provder", "srver", "hos", "opERtoR",
            "cluter",
        ];

        key_gen_types
            .iter()
            .map(|cmd| cmd.to_lowercase())
            .for_each(|cmd| {
                let gen_cmd: Cmd = clap::Parser::try_parse_from(["keys", "gen", &cmd]).unwrap();
                match gen_cmd.keys {
                    KeysCliCommand::GenCommand { keytype } => {
                        let parsed_keytype = keytype_parser(&keytype);
                        assert!(
                            parsed_keytype.is_err(),
                            "Invalid keytype parsed successfully"
                        );
                    }
                    _ => panic!("`keys gen` constructed incorrect command"),
                };
            });
    }

    #[test]
    fn test_get_basic() {
        const KEYNAME: &str = "get_basic_test.nk";
        const KEYPATH: &str = "./tests/fixtures";

        let gen_basic: Cmd =
            clap::Parser::try_parse_from(["keys", "get", KEYNAME, "--directory", KEYPATH]).unwrap();
        match gen_basic.keys {
            KeysCliCommand::GetCommand { keyname, .. } => assert_eq!(keyname, KEYNAME),
            other_cmd => panic!("keys get generated other command {other_cmd:?}"),
        }
    }

    #[test]
    /// Enumerates multiple options of the `get` command to ensure API doesn't
    /// change between versions. This test will fail if `wash keys get`
    /// changes syntax, ordering of required elements, or flags.
    fn test_get_comprehensive() {
        const KEYPATH: &str = "./tests/fixtures";
        const KEYNAME: &str = "get_comprehensive_test.nk";

        let get_all_flags: Cmd =
            clap::Parser::try_parse_from(["keys", "get", KEYNAME, "-d", KEYPATH]).unwrap();
        match get_all_flags.keys {
            KeysCliCommand::GetCommand { keyname, directory } => {
                assert_eq!(keyname, KEYNAME);
                assert_eq!(directory, Some(PathBuf::from(KEYPATH)));
            }
            other_cmd => panic!("keys get generated other command {other_cmd:?}"),
        }
    }

    #[test]
    /// Enumerates multiple options of the `list` command to ensure API doesn't
    /// change between versions. This test will fail if `wash keys list`
    /// changes syntax, ordering of required elements, or flags.
    fn test_list_comprehensive() {
        const KEYPATH: &str = "./";

        let list_basic: Cmd =
            clap::Parser::try_parse_from(["keys", "list", "-d", KEYPATH]).unwrap();
        match list_basic.keys {
            KeysCliCommand::ListCommand { .. } => (),
            other_cmd => panic!("keys get generated other command {other_cmd:?}"),
        }

        let list_all_flags: Cmd =
            clap::Parser::try_parse_from(["keys", "list", "-d", KEYPATH]).unwrap();
        match list_all_flags.keys {
            KeysCliCommand::ListCommand { directory } => {
                assert_eq!(directory, Some(PathBuf::from(KEYPATH)));
            }
            other_cmd => panic!("keys get generated other command {other_cmd:?}"),
        }
    }
}
