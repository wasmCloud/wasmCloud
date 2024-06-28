wit_bindgen::generate!();

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use exports::wasi::cli::run::Guest;
use crate::fs::PreopenedDir;
use crate::wasi::cli::environment::get_environment;
use crate::wasi::filesystem::preopens::get_directories;
use crate::wasi::sockets::instance_network::instance_network;
use crate::wasi::sockets::ip_name_lookup::{ErrorCode, resolve_addresses};
use crate::wasi::sockets::network::IpAddress;

mod fs;

struct WasiIfaceTester;

#[derive(serde::Serialize)]
struct IfaceResponse {
    args: Vec<String>,
    dirs: HashMap<String, PreopenedDir>,
    envs: HashMap<String, String>,
    name_resolution: NameResolution,
}

const DEFAULT_RESOLVED_NAME: &str = "wasmcloud.com";

#[derive(serde::Serialize)]
struct NameResolution {
    name: String,
    result: String,
    success: bool,
}

impl NameResolution {
    fn with_error(name: impl Into<String>, err: ErrorCode) -> Self {
        Self {
            name: name.into(),
            result: format!("{}", err),
            success: false,
        }
    }
    
    fn with_ip(name: impl Into<String>, ip: IpAddress) -> Self {
        let formated_ip: String = match ip {
            IpAddress::Ipv4((a, b, c, d)) => Ipv4Addr::new(a, b, c, d).to_string(),
            IpAddress::Ipv6((a, b, c, d, e, f, g, h)) => {
                Ipv6Addr::new(a, b, c, d, e, f, g, h).to_string()
            },
        };
        
        Self {
            name: name.into(),
            result: formated_ip,
            success: true,
        }
    }
    
    fn without_ip(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            result: String::default(),
            success: true,
        }
    }
}

impl Guest for WasiIfaceTester {
    fn run() -> Result<(),()> {
        let args = wasi::cli::environment::get_arguments();
        let mut dirs: HashMap<String, PreopenedDir> = HashMap::new();

        for (desc, path) in get_directories() {
            let preopened_dir = match PreopenedDir::report_descriptor(desc) {
                Ok(result) => result,
                Err(err) => {
                    eprintln!("failed to handle {}: {}", path, err);
                    return Err(());
                }
            };
            
            dirs.insert(path, preopened_dir);
        }
        
        let mut envs = HashMap::new();
        for (env_name, env_value) in get_environment() {
            envs.insert(env_name, env_value);
        }

        let name_resolution: NameResolution = match resolve_addresses(&instance_network(), DEFAULT_RESOLVED_NAME) {
            Ok(resolve_stream) => {
                resolve_stream.subscribe().block();
                match resolve_stream.resolve_next_address() {
                    Ok(ip) => {
                        match ip {
                            Some(ip) => NameResolution::with_ip(DEFAULT_RESOLVED_NAME, ip),
                            None => NameResolution::without_ip(DEFAULT_RESOLVED_NAME),
                        }
                    },
                    Err(err) => NameResolution::with_error(DEFAULT_RESOLVED_NAME, err),
                }
            },
            Err(err) => NameResolution::with_error(DEFAULT_RESOLVED_NAME, err),
        };


        let response = IfaceResponse {
            args,
            dirs,
            envs,
            name_resolution,
        };

        println!("{}", serde_json::to_string(&response).expect("failed to serialize response"));

        Ok(())
    }
}

export!(WasiIfaceTester);
