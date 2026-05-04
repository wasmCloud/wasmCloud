use anyhow::Result;
use wasmtime::{Config, Engine};

pub fn compile(wasm_bytes: &[u8]) -> Result<Vec<u8>> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)
        .map_err(|e| anyhow::anyhow!("Error setting up wasmtime engine: {e}"))?;
    let cwasm = engine
        .precompile_component(wasm_bytes)
        .map_err(|e| anyhow::anyhow!("Error precompiling wasm component: {e}"))?;
    Ok(cwasm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precompiles_a_minimal_component() {
        let wasm = wat::parse_str("(component)").unwrap();

        let cwasm = compile(&wasm).unwrap();
        assert!(!cwasm.is_empty());
    }
}
