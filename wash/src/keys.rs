use nkeys::{KeyPair, KeyPairType};
use serde_json::json;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use structopt::StructOpt;

#[derive(StructOpt, Debug, Clone)]
pub enum Output {
    Text,
    JSON,
}

impl FromStr for Output {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Output::JSON),
            "text" => Ok(Output::Text),
            _ => Err(OutputParseErr),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputParseErr;

impl Error for OutputParseErr {}

impl fmt::Display for OutputParseErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            "error parsing output type, see help for the list of accepted outputs"
        )
    }
}
#[derive(Debug, Clone, StructOpt)]
pub struct KeysCli {
    #[structopt(flatten)]
    command: KeysCliCommand,
}

#[derive(Debug, Clone, StructOpt)]
pub enum KeysCliCommand {
    #[structopt(name = "gen", about = "Generates a keypair")]
    GenCommand {
        /// The type of keypair to generate. May be Account, User, Module (Actor), Service (Capability Provider), Server, Operator, Cluster
        #[structopt(case_insensitive = true)]
        keytype: KeyPairType,
        #[structopt(
            short = "o",
            long = "output",
            default_value = "text",
            help = "Specify output format (text or json)"
        )]
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
    },
}

pub fn handle_command(cli: KeysCli) -> Result<(), Box<dyn ::std::error::Error>> {
    match cli.command {
        KeysCliCommand::GenCommand { keytype, output } => {
            println!("{}", generate(&keytype, &output));
        }
        KeysCliCommand::GetCommand { keyname, directory } => {
            get(&keyname, directory);
        }
        KeysCliCommand::ListCommand { directory } => {
            list(directory);
        }
    }
    Ok(())
}

/// Generates a keypair of the specified KeyPairType, as either Text or JSON
pub fn generate(kt: &KeyPairType, output_type: &Output) -> String {
    let kp = KeyPair::new(kt.clone());
    match output_type {
        Output::Text => format!(
            "Public Key: {}\nSeed: {}\n\nRemember that the seed is private, treat it as a secret.",
            kp.public_key(),
            kp.seed().unwrap()
        ),
        Output::JSON => json!({
            "public_key": kp.public_key(),
            "seed": kp.seed().unwrap(),
        })
        .to_string(),
    }
}

/// Retrieves a keypair by name in a specified directory, or $WASH_KEYS ($HOME/.wash/keys) if directory is not specified
pub fn get(keyname: &String, directory: Option<String>) {
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
        Ok(s) => println!("{}", s),
    }
}

/// Lists all keypairs (file extension .nk) in a specified directory or $WASH_KEYS($HOME/.wash/keys) if directory is not specified
pub fn list(directory: Option<String>) {
    let dir = determine_directory(directory);

    match fs::read_dir(dir.clone()) {
        Err(e) => println!("Error: {}, please ensure directory {} exists", e, dir),
        Ok(paths) => {
            println!("====== Keys found in {} ======\n", dir);
            for path in paths {
                let f = String::from(path.unwrap().file_name().to_str().unwrap());
                if f.ends_with(".nk") {
                    println!("{}", f);
                }
            }
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
pub fn extract_keypair(
    input: Option<String>,
    module_path: Option<String>,
    directory: Option<String>,
    keypair_type: KeyPairType,
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
        let module_name = PathBuf::from(module)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let path = format!(
            "{}/{}_{}.nk",
            dir,
            module_name,
            keypair_type_to_string(keypair_type.clone())
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
                let kp = KeyPair::new(keypair_type);
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
    use super::{generate, Output};
    use nkeys::KeyPairType;
    use serde::Deserialize;

    #[test]
    fn keys_generate_basic_test() {
        let kt = KeyPairType::Account;

        let keypair = generate(&kt, &Output::Text);
        let keypair_json = generate(&kt, &Output::JSON);

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

        let keypair_json = generate(&kt, &Output::JSON);
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
            serde_json::from_str(&generate(&KeyPairType::Account, &Output::JSON)).unwrap();
        let user_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::User, &Output::JSON)).unwrap();
        let module_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Module, &Output::JSON)).unwrap();
        let service_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Service, &Output::JSON)).unwrap();
        let server_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Server, &Output::JSON)).unwrap();
        let operator_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Operator, &Output::JSON)).unwrap();
        let cluster_keypair: KeyPairJSON =
            serde_json::from_str(&generate(&KeyPairType::Cluster, &Output::JSON)).unwrap();

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
