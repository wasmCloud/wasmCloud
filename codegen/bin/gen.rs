fn main() {
    let codegen = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "./codegen.toml".to_string());
    if let Err(e) = weld_codegen::rust_build(&codegen) {
        eprintln!("Error: running '{}': {}", &codegen, e);
    }
}
