mod bindings {
    wit_bindgen::generate!({
        world: "patch-producer",
        path: "../wit",
        generate_all,
        async: [
            "export:wasmcloud:patch-stream/patches@0.1.0#subscribe",
            "import:wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15#wait-for",
        ],
    });
}

use bindings::exports::wasmcloud::patch_stream::patches::Guest;
use bindings::wasi::clocks::monotonic_clock;
use wit_bindgen::StreamReader;

struct Component;

/// 500ms between patches — slow enough for `curl -N` to visibly render each
/// line on its own flush while debugging host-side buffering.
const TICK_NS: u64 = 500_000_000;

/// One edit session against a tiny task-list document. Each entry is a
/// complete RFC 6902 JSON Patch operation, NDJSON-framed by the writer.
const EDITS: &[&[u8]] = &[
    br#"{"op":"add","path":"/title","value":"\"Untitled\""}"#,
    br#"{"op":"add","path":"/version","value":"0"}"#,
    br#"{"op":"add","path":"/items","value":"[]"}"#,
    br#"{"op":"add","path":"/tags","value":"[]"}"#,
    br#"{"op":"replace","path":"/title","value":"\"Streaming demo\""}"#,
    br#"{"op":"add","path":"/items/0","value":"{\"id\":1,\"name\":\"draft outline\",\"done\":false}"}"#,
    br#"{"op":"add","path":"/items/1","value":"{\"id\":2,\"name\":\"prototype\",\"done\":false}"}"#,
    br#"{"op":"replace","path":"/version","value":"1"}"#,
    br#"{"op":"add","path":"/tags/0","value":"\"wasmcloud\""}"#,
    br#"{"op":"add","path":"/tags/1","value":"\"wasip3\""}"#,
    br#"{"op":"add","path":"/items/2","value":"{\"id\":3,\"name\":\"ship it\",\"done\":false}"}"#,
    br#"{"op":"replace","path":"/items/0/done","value":"true"}"#,
    br#"{"op":"replace","path":"/version","value":"2"}"#,
    br#"{"op":"replace","path":"/items/1/done","value":"true"}"#,
    br#"{"op":"add","path":"/tags/2","value":"\"poc\""}"#,
    br#"{"op":"remove","path":"/items/0"}"#,
    br#"{"op":"replace","path":"/version","value":"3"}"#,
    br#"{"op":"replace","path":"/items/0/name","value":"\"prototype (renamed)\""}"#,
    br#"{"op":"add","path":"/meta","value":"{\"emitted_by\":\"patch-producer\"}"}"#,
    br#"{"op":"replace","path":"/version","value":"4"}"#,
];

impl Guest for Component {
    async fn subscribe() -> StreamReader<u8> {
        let (mut writer, reader) = bindings::wit_stream::new::<u8>();

        wit_bindgen::spawn(async move {
            // Capture t=0 at the moment we start emitting so the
            // `[t+NNNms]` prefix on each line reflects elapsed time
            // since the writer task began, not wall-clock time.
            let start_ns = monotonic_clock::now();

            for edit in EDITS {
                let elapsed_ms = monotonic_clock::now().saturating_sub(start_ns) / 1_000_000;
                let mut line = format!("[t+{:>4}ms] ", elapsed_ms).into_bytes();
                line.extend_from_slice(edit);
                line.push(b'\n');
                writer.write_all(line).await;
                monotonic_clock::wait_for(TICK_NS).await;
            }
            // Writer drops at end of scope → stream closes → consumer's
            // HTTP body finishes → curl exits cleanly.
        });

        reader
    }
}

bindings::export!(Component with_types_in bindings);
