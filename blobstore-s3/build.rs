const CONFIG: &str = "./codegen.toml";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    weld_codegen::rust_build_into(CONFIG, &std::env::var("OUT_DIR").unwrap())?;
    Ok(())
}
