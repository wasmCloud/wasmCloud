use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_lattice_controller::LatticeControllerProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    wasmcloud_provider_sdk::start_provider(
        LatticeControllerProvider::with_cache_timeout_minutes(600),
        Some("lattice-control-provider".to_string()),
    )?;

    eprintln!("Lattice Controller capability provider exiting");
    Ok(())
}
