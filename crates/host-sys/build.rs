use std::env;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let crate_dir =
        env::var("CARGO_MANIFEST_DIR").context("failed to lookup `CARGO_MANIFEST_DIR`")?;
    let mut config: cbindgen::Config = Default::default();
    config.language = cbindgen::Language::C;
    let bindings =
        cbindgen::generate_with_config(crate_dir, config).context("failed to generate bindings")?;
    let out_dir = env::var("OUT_DIR").context("failed to lookup `OUT_DIR`")?;
    bindings.write_to_file(format!("{out_dir}/host.h"));
    Ok(())
}
