// TODO(brooksmtownsend): bring the lattice control capability provider up-to-date with the control interface
// use wasmcloud_provider_lattice_controller::LatticeControllerProvider;

fn main() -> anyhow::Result<()> {
    // wasmcloud_provider_sdk::start_provider(
    //     LatticeControllerProvider::with_cache_timeout_minutes(600),
    //     Some("lattice-control-provider".to_string()),
    // )?;

    eprintln!("Lattice Controller capability provider exiting");
    Ok(())
}
