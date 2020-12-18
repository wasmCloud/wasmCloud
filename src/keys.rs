use crate::util::{format_output, Output, OutputKind};
use nkeys::{KeyPair, KeyPairType};
use serde_json::json;
use std::env;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use structopt::StructOpt;

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct KeysCli {
    #[structopt(flatten)]
    command: KeysCliCommand,
}

#[derive(Debug, Clone, StructOpt)]
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

pub(crate) fn handle_command(cli: KeysCli) -> Result<(), Box<dyn ::std::error::Error>> {
    match cli.command {
        KeysCliCommand::GenCommand { keytype, output } => {
            println!("{}", generate(&keytype, &output.kind));
        }
        KeysCliCommand::GetCommand {
            keyname,
            directory,
            output,
        } => {
            get(&keyname, directory, &output);
        }
        KeysCliCommand::ListCommand { directory, output } => {
            list(directory, &output);
        }
    }
    Ok(())
}

/// Generates a keypair of the specified KeyPairType, as either Text or JSON
pub(crate) fn generate(kt: &KeyPairType, output: &OutputKind) -> String {
    let kp = KeyPair::new(kt.clone());
    match output {
        OutputKind::Text => format!(
            "Public Key: {}\nSeed: {}\n\nRemember that the seed is private, treat it as a secret.",
            kp.public_key(),
            kp.seed().unwrap()
        ),
        OutputKind::JSON => json!({
            "public_key": kp.public_key(),
            "seed": kp.seed().unwrap(),
        })
        .to_string(),
    }
}

/// Retrieves a keypair by name in a specified directory, or $WASH_KEYS ($HOME/.wash/keys) if directory is not specified
pub(crate) fn get(keyname: &String, directory: Option<String>, output: &Output) {
    let dir = determine_directory(directory);
    let mut f = match File::open(format!("{}/{}", dir, keyname)) {
        Ok(f) => f,
        Err(f) => {
            println!("Error: {}.\nPlease ensure {}/{} exists.", f, dir, keyname);
            return;
        }
    };
    let mut s = String::new();
    let res = match f.read_to_string(&mut s) {
        Ok(_) => Ok(s),
        Err(e) => Err(e),
    };
    match res {
        Err(e) => println!("Error: {:?}", e.kind()),
        Ok(s) => println!(
            "{}",
            format_output(s.clone(), json!({ "seed": s }), &output.kind)
        ),
    }
}

/// Lists all keypairs (file extension .nk) in a specified directory or $WASH_KEYS($HOME/.wash/keys) if directory is not specified
pub(crate) fn list(directory: Option<String>, output: &Output) {
    let dir = determine_directory(directory);

    let mut keys = vec![];
    match fs::read_dir(dir.clone()) {
        Err(e) => println!("Error: {}, please ensure directory {} exists", e, dir),
        Ok(paths) => {
            for path in paths {
                let f = String::from(path.unwrap().file_name().to_str().unwrap());
                if f.ends_with(".nk") {
                    keys.push(f);
                }
            }
        }
    }

    match output.kind {
        OutputKind::Text => {
            println!("====== Keys found in {} ======\n", dir);
            for key in keys {
                println!("{}", key);
            }
        }
        OutputKind::JSON => {
            println!("{}", json!({ "keys": keys }))
        }
    }
}

fn determine_directory(directory: Option<String>) -> String {
    match directory {
        Some(d) => d,
        None => format!("{}/.wash/keys", env::var("HOME").unwrap()),
    }
}

/// Helper function to locate and extract keypair from user input
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
        let dir = determine_directory(directory);
        // Account key should be re-used, and will attempt to generate based on the terminal USER
        let module_name = match keygen_type {
            KeyPairType::Account => std::env::var("USER").unwrap_or("user".to_string()),
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
                let kp = KeyPair::new(keygen_type);
                println!("No keypair found in \"{}\".\nWe will generate one for you and place it there.\nIf you'd like to use alternative keys, you can supply them as a flag.\n", path);
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
    use super::{generate, OutputKind};
    use nkeys::KeyPairType;
    use serde::Deserialize;

    #[test]
    fn keys_generate_basic_test() {
        let kt = KeyPairType::Account;

        let keypair = generate(&kt, &OutputKind::Text);
        let keypair_json = generate(&kt, &OutputKind::JSON);

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
    struct KeyPairJSON {
        public_key: String,
        seed: String,
    }

    #[test]
    fn keys_generate_valid_keypair() {
        let sample_public_key = "MBBLAHS7MCGNQ6IR4ZDSGRIAF7NVS7FCKFTKGO5JJJKN2QQRVAH7BSIO";
        let sample_seed = "SMAH45IUULL57OSX23NOOOTLSVNQOORMDLE3Y3PQLJ4J5MY7MN2K7BIFI4";

        let kt = KeyPairType::Module;

        let keypair_json = generate(&kt, &OutputKind::JSON);
        let keypair: KeyPairJSON = serde_json::from_str(&keypair_json).unwrap();

        assert_eq!(keypair.public_key.len(), sample_public_key.len());
        assert_eq!(keypair.seed.len(), sample_seed.len());
        assert_eq!(keypair.public_key.starts_with("M"), true);
        assert_eq!(keypair.seed.starts_with("SM"), true);
    }

    #[test]
    fn keys_generate_all_types() {
        let sample_public_key = "MBBLAHS7MCGNQ6IR4ZDSGRIAF7NVS7FCKFTKGO5JJJKN2QQRVAH7BSIO";
        let sample_seed = "SMAH45IUULL57OSXNOOAKOTLSVNQOORMDLE3Y3PQLJ4J5MY7MN2K7BIFI4";

        let account_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Account, &OutputKind::JSON)).unwrap();
        let user_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::User, &OutputKind::JSON)).unwrap();
        let module_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Module, &OutputKind::JSON)).unwrap();
        let service_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Service, &OutputKind::JSON)).unwrap();
        let server_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Server, &OutputKind::JSON)).unwrap();
        let operator_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Operator, &OutputKind::JSON)).unwrap();
        let cluster_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Cluster, &OutputKind::JSON)).unwrap();

        assert_eq!(account_keypair.public_key.starts_with("A"), true);
        assert_eq!(account_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(account_keypair.seed.starts_with("SA"), true);
        assert_eq!(account_keypair.seed.len(), sample_seed.len());

        assert_eq!(user_keypair.public_key.starts_with("U"), true);
        assert_eq!(user_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(user_keypair.seed.starts_with("SU"), true);
        assert_eq!(user_keypair.seed.len(), sample_seed.len());

        assert_eq!(module_keypair.public_key.starts_with("M"), true);
        assert_eq!(module_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(module_keypair.seed.starts_with("SM"), true);
        assert_eq!(module_keypair.seed.len(), sample_seed.len());

        assert_eq!(service_keypair.public_key.starts_with("V"), true);
        assert_eq!(service_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(service_keypair.seed.starts_with("SV"), true);
        assert_eq!(service_keypair.seed.len(), sample_seed.len());

        assert_eq!(server_keypair.public_key.starts_with("N"), true);
        assert_eq!(server_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(server_keypair.seed.starts_with("SN"), true);
        assert_eq!(server_keypair.seed.len(), sample_seed.len());

        assert_eq!(operator_keypair.public_key.starts_with("O"), true);
        assert_eq!(operator_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(operator_keypair.seed.starts_with("SO"), true);
        assert_eq!(operator_keypair.seed.len(), sample_seed.len());

        assert_eq!(cluster_keypair.public_key.starts_with("C"), true);
        assert_eq!(cluster_keypair.public_key.len(), sample_public_key.len());
        assert_eq!(cluster_keypair.seed.starts_with("SC"), true);
        assert_eq!(cluster_keypair.seed.len(), sample_seed.len());
    }
}
