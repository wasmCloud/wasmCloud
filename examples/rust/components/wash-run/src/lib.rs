wit_bindgen::generate!();

use std::collections::HashMap;
use exports::wasi::cli::run::Guest;
use crate::fs::PreopenedDir;
use crate::wasi::filesystem::preopens::get_directories;

mod fs;

struct WasiIfaceTester;

#[derive(serde::Serialize)]
struct IfaceResponse {
    args: Vec<String>,
    dirs: HashMap<String, PreopenedDir>,
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

        let response = IfaceResponse {
            args,
            dirs,
        };

        println!("{}", serde_json::to_string(&response).expect("failed to serialize response"));

        Ok(())
    }
}

export!(WasiIfaceTester);
