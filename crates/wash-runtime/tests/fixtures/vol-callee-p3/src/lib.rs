//! Same `gate` export as ctx-race-callee-p3 (a future-carrying call, so it runs
//! on the shared-store path), but `hold` reports this component's preopens and
//! whether a write under its read-only `/workspace` mount was refused:
//! `<preopens joined by +>;write=<refused|ALLOWED|...>`.
mod bindings {
    wit_bindgen::generate!({ world: "vol-callee", generate_all });
}
use bindings::exports::wasmcloud::ctx_race::gate::Guest;
use wit_bindgen::FutureReader;
struct Component;
impl Guest for Component {
    async fn hold(release: FutureReader<u32>) -> String {
        let _ = release.await;
        let preopens = bindings::wasi::filesystem::preopens::get_directories()
            .into_iter()
            .map(|(_, p)| p)
            .collect::<Vec<_>>()
            .join("+");
        let write = match std::fs::write("/workspace/probe-write", b"x") {
            Ok(()) => "ALLOWED".to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => "refused".to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::ReadOnlyFilesystem => "refused".to_string(),
            Err(e) => format!("{:?}", e.kind()),
        };
        format!("{preopens};write={write}")
    }
}
bindings::export!(Component with_types_in bindings);
