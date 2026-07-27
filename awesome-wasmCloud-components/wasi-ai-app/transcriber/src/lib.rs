
mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:wasi3-ai-app/producer@0.1.0#produce",
        ],
    });
}

use bindings::exports::wasmcloud::wasi3_ai_app::producer::Guest;
use wit_bindgen::StreamReader;
use wit_bindgen::spawn_local;
use oxiwhisper::{TranscribeOptions, WhisperModel};
use std::path::Path;
use tracing::{debug, info};

struct Component;

impl Guest for Component {
    async fn produce() -> StreamReader<u8> {

        tracing_subscriber::fmt()
            .with_env_filter("info")
            .with_writer(std::io::stderr) // or stdout
            .init();
        
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();

        spawn_local(async move {

            info!("Audio transcriber backend started");
            
            let model = WhisperModel::from_file(Path::new("data/ggml-tiny.bin")).unwrap();
            
            let audio = oxiwhisper::audio::load_wav(Path::new("data/sample_audio.wav")).unwrap();
            
            info!("AI Model: ggml-tiny.bin");
            info!("Audio File as Input: output.wav");
            
            let opts = TranscribeOptions {
                timestamps: true,
                ..TranscribeOptions::default()
            };

            let mut stream = model.stream(opts);
            stream.push_audio(&audio);
            
            info!("Segmented Transcription starting...");
            // 4. Retrieve segments (each contains timestamps)
            while let Some(result) = stream.next_segment() {
                match result {
                    Ok(segment) => {

                        info!(start = segment.start , end = segment.end, segment = segment.text);
                        
                        tx.write_all(segment.text.into_bytes()).await;
                        
                    }
                    Err(e) => eprintln!("Error: {e}"),
                }
            } 

            info!("Segmented Transcription end.");
            
            if stream.next_segment().is_none() {
                debug!("Stream next_segment is None");
            }
            // let result = stream.finish().unwrap();

            //tx.write_all(text.into_bytes()).await;
            info!("*****************************************");
            drop(tx);
        });
        rx
    }
}

bindings::export!(Component with_types_in bindings);
