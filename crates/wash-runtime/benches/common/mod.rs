const HTTP_HANDLER_P2_WASM: &[u8] = include_bytes!("../../tests/wasm/http_handler_p2.wasm");
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("../../tests/wasm/http_handler_p3.wasm");

#[derive(Copy, Clone, Debug)]
pub enum Flavor {
    P2,
    P3,
}

impl Flavor {
    pub fn name(self) -> &'static str {
        match self {
            Flavor::P2 => "p2",
            Flavor::P3 => "p3",
        }
    }

    pub fn wasm(self) -> &'static [u8] {
        match self {
            Flavor::P2 => HTTP_HANDLER_P2_WASM,
            Flavor::P3 => HTTP_HANDLER_P3_WASM,
        }
    }

    pub fn expected_body(self) -> &'static str {
        match self {
            Flavor::P2 => "hello from p2",
            Flavor::P3 => "hello from p3",
        }
    }
}
