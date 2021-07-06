// build.rs - build smithy models into rust sources at compile tile

// path to codegen.toml relative to location of Cargo.toml
const CONFIG: &str = "../codegen.toml";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    weld_codegen::rust_build(CONFIG)?;
    Ok(())
}
