use std::time::SystemTime;

use ferris_says::say;

wit_bindgen::generate!({ generate_all });

use exports::wasmcloud::example_ferris_says::invoke::Guest;
use wasi::clocks;

const MAX_TERM_WIDTH: usize = 80;

/// Implementation for the exported functionality on the WIT world hangs
/// from this struct.
struct FerrisSayer;

impl Guest for FerrisSayer {
    fn say() -> String {
        // NOTE: using the "SystemTime" standard library API means using
        // wasi:clocks when this Rust code is compiled down to WebAssembly + WASI
        let now = SystemTime::now();
        let now_monotonic = clocks::monotonic_clock::now();
        make_ferris_say(&format!(
            "Hello fellow wasmCloud users! (@{}| {now_monotonic})",
            humantime::format_rfc3339(now),
        ))
    }

    fn say_phrase(phrase: String) -> String {
        make_ferris_say(&phrase)
    }
}

/// Reusable functionality for calling  funcitonality
fn make_ferris_say(phrase: &str) -> String {
    let mut w = Vec::new();
    if let Err(e) = say(phrase, MAX_TERM_WIDTH, &mut w) {
        return format!("ERROR: internal error, failed to say: {e}");
    };
    match String::from_utf8(w) {
        Ok(s) => s,
        Err(_) => "ERROR: only UTF8 strings are allowed as input".into(),
    }
}

export!(FerrisSayer);
