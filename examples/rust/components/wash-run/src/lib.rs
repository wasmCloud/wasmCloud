use std::collections::HashMap;

use exports::wasi::cli::run::Guest;

use crate::wasi::filesystem::preopens::get_directories;

mod fs;

wit_bindgen::generate!();

struct WasiIfaceTester;

#[derive(serde::Serialize)]
struct IfaceResponse {
    args: Vec<String>,
    dirs: HashMap<String, fs::PreopenedDir>,
}

impl Guest for WasiIfaceTester {
    fn run() -> Result<(),()> {
        let args = wasi::cli::environment::get_arguments();
        let mut dirs: HashMap<String, fs::PreopenedDir> = HashMap::new();

        for (desc, path) in get_directories() {
            dirs.insert(path, desc.try_into().unwrap());
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
