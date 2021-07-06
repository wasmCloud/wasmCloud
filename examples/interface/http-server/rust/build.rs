const CONFIG: &str = "../codegen.toml";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    weld_codegen::rust_build(CONFIG)?;
    Ok(())
}
