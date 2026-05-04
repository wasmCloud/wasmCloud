use wasmtime::{Config, Engine, error};

pub fn compile(wasm_bytes: &[u8]) -> error::Result<Vec<u8>> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config)?;
    let cwasm = engine.precompile_component(wasm_bytes)?;
    Ok(cwasm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(unsafe_code)]
    fn precompiles_a_minimal_component() {
        let wasm = wat::parse_str("(component)").unwrap();

        let cwasm = compile(&wasm).unwrap();
        assert!(!cwasm.is_empty());
    }
}
