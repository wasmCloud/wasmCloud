use crate::{
    cfg::cfg_dir,
    util::{set_permissions_keys, CommandOutput, OutputKind},
};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use nkeys::{KeyPair, KeyPairType};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    fs::File,
    io::prelude::*,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Subcommand)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum KeysCliCommand {
    #[clap(name = "gen", about = "Generates a keypair")]
    GenCommand {
        /// The type of keypair to generate. May be Account, User, Module (Actor), Service (Capability Provider), Server, Operator, Cluster
        #[clap(ignore_case = true)]
        keytype: KeyPairType,
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

pub(crate) fn handle_command(command: KeysCliCommand) -> Result<CommandOutput> {
    match command {
        KeysCliCommand::GenCommand { keytype } => generate(&keytype),
        KeysCliCommand::GetCommand { keyname, directory } => get(&keyname, directory),
        KeysCliCommand::ListCommand { directory } => list(directory),
    }
}

/// Generates a keypair of the specified KeyPairType
pub(crate) fn generate(kt: &KeyPairType) -> Result<CommandOutput> {
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

/// Retrieves a keypair by name in a specified directory, or $WASH_KEYS ($HOME/.wash/keys) if directory is not specified
pub(crate) fn get(keyname: &str, directory: Option<PathBuf>) -> Result<CommandOutput> {
    let keyfile = determine_directory(directory)?.join(keyname);
    let mut f = File::open(&keyfile)
        .with_context(|| format!("Please ensure {} exists.", keyfile.display()))?;

    let mut s = String::new();
    let seed = match f.read_to_string(&mut s) {
        Ok(_) => Ok(s),
        Err(e) => Err(e),
    }?;

    Ok(CommandOutput::from_key_and_text("seed", seed.trim()))
}

/// Lists all keypairs (file extension .nk) in a specified directory or $WASH_KEYS($HOME/.wash/keys) if directory is not specified
pub(crate) fn list(directory: Option<PathBuf>) -> Result<CommandOutput> {
    let dir = determine_directory(directory)?;

    let mut keys = vec![];
    let paths = fs::read_dir(dir.clone())
        .with_context(|| format!("please ensure directory {} exists", dir.display()))?;

    for path in paths {
        let f = String::from(path.unwrap().file_name().to_str().unwrap());
        if f.ends_with(".nk") {
            keys.push(f);
        }
    }

    let mut map = HashMap::new();
    map.insert("keys".to_string(), json!(keys));
    Ok(CommandOutput::new(
        format!(
            "====== Keys found in {} ======\n{}",
            dir.display(),
            keys.join("\n")
        ),
        map,
    ))
}

fn determine_directory(directory: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = directory {
        Ok(d)
    } else {
        let d = cfg_dir()?.join("keys");
        Ok(d)
    }
}

/// Helper function to locate and extract keypair from user input
/// Returns a tuple of the keypair and optional autogenerate message
pub(crate) fn extract_keypair(
    input: Option<String>,
    module_path: Option<String>,
    directory: Option<PathBuf>,
    keygen_type: KeyPairType,
    disable_keygen: bool,
    output_kind: OutputKind,
) -> Result<KeyPair> {
    let seed = if let Some(input_str) = input {
        match File::open(input_str.clone()) {
            // User provided file path to seed as argument
            Ok(mut f) => {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                s
            }
            // User provided seed as an argument
            Err(_e) => input_str,
        }
    } else if let Some(module) = module_path {
        // No seed value provided, attempting to source from provided or default directory
        let dir = determine_directory(directory)?;
        // Account key should be re-used, and will attempt to generate based on the terminal USER
        let module_name = match keygen_type {
            KeyPairType::Account => std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
            _ => PathBuf::from(module)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        };
        let path = dir.join(format!(
            "{}_{}.nk",
            module_name,
            keypair_type_to_string(keygen_type.clone())
        ));
        match File::open(path.clone()) {
            // Default key found
            Ok(mut f) => {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                s
            }
            // No default key, generating for user
            Err(_e) if !disable_keygen => {
                match output_kind {
                    OutputKind::Text => println!(
                        "No keypair found in \"{}\".
                    We will generate one for you and place it there.
                    If you'd like to use alternative keys, you can supply them as a flag.\n",
                        path.display()
                    ),
                    OutputKind::Json => {
                        println!(
                            "{}",
                            json!({"status": "No keypair found", "path": path, "keygen": "true"})
                        )
                    }
                }

                let kp = KeyPair::new(keygen_type);
                let seed = kp.seed()?;
                let key_path = Path::new(&path).parent().unwrap();
                fs::create_dir_all(key_path)?;
                set_permissions_keys(key_path)?;
                let mut f = File::create(path.clone())?;
                f.write_all(seed.as_bytes())?;
                set_permissions_keys(&path)?;
                seed
            }
            _ => {
                bail!(
                    "No keypair found in {}, please ensure key exists or supply one as a flag",
                    path.display()
                );
            }
        }
    } else {
        bail!("Keypair path or string not supplied. Ensure provided keypair is valid");
    };

    Ok(KeyPair::from_seed(&seed)?)
}

fn keypair_type_to_string(keypair_type: KeyPairType) -> String {
    use KeyPairType::*;
    match keypair_type {
        Account => "account".to_string(),
        Cluster => "cluster".to_string(),
        Service => "service".to_string(),
        Module => "module".to_string(),
        Server => "server".to_string(),
        Operator => "operator".to_string(),
        User => "user".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{generate, KeysCliCommand};
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
        let key_gen_types = vec![
            "account", "user", "module", "service", "server", "operator", "cluster",
        ];

        key_gen_types.iter().for_each(|cmd| {
            let gen_cmd: Cmd = clap::Parser::try_parse_from(["keys", "gen", cmd]).unwrap();
            match gen_cmd.keys {
                KeysCliCommand::GenCommand { keytype } => {
                    use KeyPairType::*;
                    match keytype {
                        Account => assert_eq!(*cmd, "account"),
                        User => assert_eq!(*cmd, "user"),
                        Module => assert_eq!(*cmd, "module"),
                        Service => assert_eq!(*cmd, "service"),
                        Server => assert_eq!(*cmd, "server"),
                        Operator => assert_eq!(*cmd, "operator"),
                        Cluster => assert_eq!(*cmd, "cluster"),
                    }
                }
                _ => panic!("`keys gen` constructed incorrect command"),
            };
        });

        key_gen_types.iter().for_each(|cmd| {
            let gen_cmd: Cmd = clap::Parser::try_parse_from(["keys", "gen", cmd]).unwrap();
            match gen_cmd.keys {
                KeysCliCommand::GenCommand { keytype } => {
                    use KeyPairType::*;
                    match keytype {
                        Account => assert_eq!(*cmd, "account"),
                        User => assert_eq!(*cmd, "user"),
                        Module => assert_eq!(*cmd, "module"),
                        Service => assert_eq!(*cmd, "service"),
                        Server => assert_eq!(*cmd, "server"),
                        Operator => assert_eq!(*cmd, "operator"),
                        Cluster => assert_eq!(*cmd, "cluster"),
                    }
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
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
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
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
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
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
        }

        let list_all_flags: Cmd =
            clap::Parser::try_parse_from(["keys", "list", "-d", KEYPATH]).unwrap();
        match list_all_flags.keys {
            KeysCliCommand::ListCommand { directory } => {
                assert_eq!(directory, Some(PathBuf::from(KEYPATH)));
            }
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
        }
    }
}
