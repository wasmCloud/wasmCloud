use crate::util::{format_output, Output, OutputKind};
use nkeys::{KeyPair, KeyPairType};
use serde_json::json;
use std::env;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::io::Error;
use std::path::{Path, PathBuf};
use structopt::StructOpt;

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct KeysCli {
    #[structopt(flatten)]
    command: KeysCliCommand,
}

impl KeysCli {
    pub(crate) fn command(self) -> KeysCliCommand {
        self.command
    }
}

#[derive(Debug, Clone, StructOpt)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum KeysCliCommand {
    #[structopt(name = "gen", about = "Generates a keypair")]
    GenCommand {
        /// The type of keypair to generate. May be Account, User, Module (Actor), Service (Capability Provider), Server, Operator, Cluster
        #[structopt(case_insensitive = true)]
        keytype: KeyPairType,
        #[structopt(flatten)]
        output: Output,
    },
    #[structopt(name = "get", about = "Retrieves a keypair and prints the contents")]
    GetCommand {
        #[structopt(help = "The name of the key to output")]
        keyname: String,
        #[structopt(
            short = "d",
            long = "directory",
            env = "WASH_KEYS",
            hide_env_values = true,
            help = "Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`"
        )]
        directory: Option<String>,
        #[structopt(flatten)]
        output: Output,
    },
    #[structopt(name = "list", about = "Lists all keypairs in a directory")]
    ListCommand {
        #[structopt(
            short = "d",
            long = "directory",
            env = "WASH_KEYS",
            hide_env_values = true,
            help = "Absolute path to where keypairs are stored. Defaults to `$HOME/.wash/keys`"
        )]
        directory: Option<String>,
        #[structopt(flatten)]
        output: Output,
    },
}

pub(crate) fn handle_command(
    command: KeysCliCommand,
) -> Result<String, Box<dyn ::std::error::Error>> {
    match command {
        KeysCliCommand::GenCommand { keytype, output } => Ok(generate(&keytype, &output.kind)),
        KeysCliCommand::GetCommand {
            keyname,
            directory,
            output,
        } => get(&keyname, directory, &output),
        KeysCliCommand::ListCommand { directory, output } => list(directory, &output),
    }
}

/// Generates a keypair of the specified KeyPairType, as either Text or JSON
pub(crate) fn generate(kt: &KeyPairType, output: &OutputKind) -> String {
    let kp = KeyPair::new(kt.clone());
    format_output(
        format!(
            "Public Key: {}\nSeed: {}\n\nRemember that the seed is private, treat it as a secret.",
            kp.public_key(),
            kp.seed().unwrap()
        ),
        json!({
            "public_key": kp.public_key(),
            "seed": kp.seed().unwrap(),
        }),
        output,
    )
}

/// Retrieves a keypair by name in a specified directory, or $WASH_KEYS ($HOME/.wash/keys) if directory is not specified
pub(crate) fn get(
    keyname: &str,
    directory: Option<String>,
    output: &Output,
) -> Result<String, Box<dyn ::std::error::Error>> {
    let dir = determine_directory(directory)?;
    let mut f = File::open(format!("{}/{}", dir, keyname))
        .map_err(|e| format!("{}.\nPlease ensure {}/{} exists.", e, dir, keyname))?;

    let mut s = String::new();
    let res = match f.read_to_string(&mut s) {
        Ok(_) => Ok(s),
        Err(e) => Err(e),
    };
    match res {
        Err(e) => Err(e.into()),
        Ok(s) => Ok(format_output(
            s.trim().to_string(),
            json!({ "seed": s.trim() }),
            &output.kind,
        )),
    }
}

/// Lists all keypairs (file extension .nk) in a specified directory or $WASH_KEYS($HOME/.wash/keys) if directory is not specified
pub(crate) fn list(
    directory: Option<String>,
    output: &Output,
) -> Result<String, Box<dyn ::std::error::Error>> {
    let dir = determine_directory(directory)?;

    let mut keys = vec![];
    let paths = fs::read_dir(dir.clone())
        .map_err(|e| format!("Error: {}, please ensure directory {} exists", e, dir))?;

    for path in paths {
        let f = String::from(path.unwrap().file_name().to_str().unwrap());
        if f.ends_with(".nk") {
            keys.push(f);
        }
    }

    Ok(format_output(
        format!("====== Keys found in {} ======\n{}", dir, keys.join("\n")),
        json!({ "keys": keys }),
        &output.kind,
    ))
}

fn determine_directory(directory: Option<String>) -> Result<String, Error> {
    if let Some(d) = directory {
        Ok(d)
    } else if let Ok(home) = env::var("HOME") {
        Ok(format!("{}/.wash/keys", home))
    } else {
        Err(Error::new(
            std::io::ErrorKind::NotFound,
            "$HOME not found, please set $HOME or $WASH_KEYS for autogenerated keys".to_string(),
        ))
    }
}

/// Helper function to locate and extract keypair from user input
/// Returns a tuple of the keypair and optional autogenerate message
pub(crate) fn extract_keypair(
    input: Option<String>,
    module_path: Option<String>,
    directory: Option<String>,
    keygen_type: KeyPairType,
    disable_keygen: bool,
) -> Result<KeyPair, Box<dyn std::error::Error>> {
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
        let path = format!(
            "{}/{}_{}.nk",
            dir,
            module_name,
            keypair_type_to_string(keygen_type.clone())
        );
        match File::open(path.clone()) {
            // Default key found
            Ok(mut f) => {
                let mut s = String::new();
                f.read_to_string(&mut s)?;
                s
            }
            // No default key, generating for user
            Err(_e) if !disable_keygen => {
                println!(
                    "{}",
                    crate::util::format_output(
                        format!(
                            "No keypair found in \"{}\".
We will generate one for you and place it there.
If you'd like to use alternative keys, you can supply them as a flag.\n",
                            path
                        ),
                        json!({"status": "No keypair found", "path": path, "keygen": "true"}),
                        &Output::default().kind,
                    )
                );

                let kp = KeyPair::new(keygen_type);
                let seed = kp.seed()?;
                fs::create_dir_all(Path::new(&path).parent().unwrap())?;
                let mut f = File::create(path)?;
                f.write_all(seed.as_bytes())?;
                seed
            }
            _ => {
                return Err(format!(
                    "No keypair found in {}, please ensure key exists or supply one as a flag",
                    path
                )
                .into());
            }
        }
    } else {
        return Err("Keypair path or string not supplied. Ensure provided keypair is valid".into());
    };

    KeyPair::from_seed(&seed).map_err(|e| format!("{}", e).into())
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
    use super::{generate, KeysCli, KeysCliCommand, OutputKind};
    use nkeys::KeyPairType;
    use serde::Deserialize;
    use structopt::StructOpt;

    #[test]
    fn test_generate_basic_test() {
        let kt = KeyPairType::Account;

        let keypair = generate(&kt, &OutputKind::Text);
        let keypair_json = generate(&kt, &OutputKind::Json);

        assert_eq!(keypair.contains("Public Key: "), true);
        assert_eq!(keypair.contains("Seed: "), true);
        assert_eq!(
            keypair.contains("Remember that the seed is private, treat it as a secret."),
            true
        );
        assert_ne!(keypair, "");
        assert_ne!(keypair_json, "");
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

        let keypair_json = generate(&kt, &OutputKind::Json);
        let keypair: KeyPairJson = serde_json::from_str(&keypair_json).unwrap();

        assert_eq!(keypair.public_key.len(), sample_public_key.len());
        assert_eq!(keypair.seed.len(), sample_seed.len());
        assert_eq!(keypair.public_key.starts_with('M'), true);
        assert_eq!(keypair.seed.starts_with("SM"), true);
    }

    #[test]
    fn test_generate_all_types() {
        let sample_public_key = "MBBLAHS7MCGNQ6IR4ZDSGRIAF7NVS7FCKFTKGO5JJJKN2QQRVAH7BSIO";
        let sample_seed = "SMAH45IUULL57OSXNOOAKOTLSVNQOORMDLE3Y3PQLJ4J5MY7MN2K7BIFI4";

        let account_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Account, &OutputKind::Json)).unwrap();
        let user_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::User, &OutputKind::Json)).unwrap();
        let module_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Module, &OutputKind::Json)).unwrap();
        let service_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Service, &OutputKind::Json)).unwrap();
        let server_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Server, &OutputKind::Json)).unwrap();
        let operator_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Operator, &OutputKind::Json)).unwrap();
        let cluster_keypair: KeyPairJson =
            serde_json::from_str(&generate(&KeyPairType::Cluster, &OutputKind::Json)).unwrap();

        assert_eq!(account_keypair.public_key.starts_with('A'), true);
        assert_eq!(account_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(account_keypair.seed.starts_with("SA"), true);
        assert_eq!(account_keypair.seed.len(), sample_seed.len());

        assert_eq!(user_keypair.public_key.starts_with('U'), true);
        assert_eq!(user_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(user_keypair.seed.starts_with("SU"), true);
        assert_eq!(user_keypair.seed.len(), sample_seed.len());

        assert_eq!(module_keypair.public_key.starts_with('M'), true);
        assert_eq!(module_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(module_keypair.seed.starts_with("SM"), true);
        assert_eq!(module_keypair.seed.len(), sample_seed.len());

        assert_eq!(service_keypair.public_key.starts_with('V'), true);
        assert_eq!(service_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(service_keypair.seed.starts_with("SV"), true);
        assert_eq!(service_keypair.seed.len(), sample_seed.len());

        assert_eq!(server_keypair.public_key.starts_with('N'), true);
        assert_eq!(server_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(server_keypair.seed.starts_with("SN"), true);
        assert_eq!(server_keypair.seed.len(), sample_seed.len());

        assert_eq!(operator_keypair.public_key.starts_with('O'), true);
        assert_eq!(operator_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(operator_keypair.seed.starts_with("SO"), true);
        assert_eq!(operator_keypair.seed.len(), sample_seed.len());

        assert_eq!(cluster_keypair.public_key.starts_with('C'), true);
        assert_eq!(cluster_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(cluster_keypair.seed.starts_with("SC"), true);
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
            let gen_cmd = KeysCli::from_iter(&["keys", "gen", cmd]);
            match gen_cmd.command {
                KeysCliCommand::GenCommand { keytype, output } => {
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
                    assert_eq!(output.kind, OutputKind::Text);
                }
                _ => panic!("`keys gen` constructed incorrect command"),
            };
        });

        key_gen_types.iter().for_each(|cmd| {
            let gen_cmd = KeysCli::from_iter(&["keys", "gen", cmd, "-o", "json"]);
            match gen_cmd.command {
                KeysCliCommand::GenCommand { keytype, output } => {
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
                    assert_eq!(output.kind, OutputKind::Json);
                }
                _ => panic!("`keys gen` constructed incorrect command"),
            };
        });
    }

    #[test]
    fn test_get_basic() {
        const KEYNAME: &str = "get_basic_test.nk";
        const KEYPATH: &str = "./tests/fixtures";

        let get_basic = KeysCli::from_iter(&["keys", "get", KEYNAME, "--directory", KEYPATH]);
        match get_basic.command {
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

        let get_all_flags =
            KeysCli::from_iter(&["keys", "get", KEYNAME, "-d", KEYPATH, "-o", "json"]);
        match get_all_flags.command {
            KeysCliCommand::GetCommand {
                keyname,
                directory,
                output,
            } => {
                assert_eq!(keyname, KEYNAME);
                assert_eq!(directory, Some(KEYPATH.to_string()));
                assert_eq!(output.kind, OutputKind::Json);
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

        let list_basic = KeysCli::from_iter(&["keys", "list", "-d", KEYPATH]);
        match list_basic.command {
            KeysCliCommand::ListCommand { .. } => (),
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
        }

        let list_all_flags = KeysCli::from_iter(&["keys", "list", "-d", KEYPATH, "-o", "json"]);
        match list_all_flags.command {
            KeysCliCommand::ListCommand { directory, output } => {
                assert_eq!(directory, Some(KEYPATH.to_string()));
                assert_eq!(output.kind, OutputKind::Json);
            }
            other_cmd => panic!("keys get generated other command {:?}", other_cmd),
        }
    }
}
