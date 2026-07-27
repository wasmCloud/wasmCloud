mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "export:wasmcloud:wasi3-ai-app/summarizer@0.1.0#summarize",
        ],
    });
}

pub mod ai_model_prompts;
pub mod token_output_stream;

use ai_model_prompts::configure_prompts;
use bindings::exports::wasmcloud::wasi3_ai_app::summarizer::Guest;
use candle::Tensor;
use candle::quantized::gguf_file;
use candle_core as candle;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::quantized_qwen3::ModelWeights as Qwen3;
use token_output_stream::{TokenOutputStream, device};
use tokenizers::Tokenizer;
use tracing::{error, info};
use wit_bindgen::StreamReader;

const MODEL_RELATIVE_PATH: &str = "Qwen3-0.6B-Q4_K_M/Qwen3-0.6B-Q4_K_M.gguf";
const TOKENIZER_RELATIVE_PATH: &str = "Qwen3-0.6B-Q4_K_M/tokenizer.json";

fn resolve_data_path(relative_path: &str) -> std::path::PathBuf {
    let workspace_root = std::path::PathBuf::from("/");

    let candidates = [
        workspace_root.join("data").join(relative_path),
        workspace_root.join(relative_path),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| workspace_root.join("../testdata").join(relative_path))
}

struct Component;

impl Guest for Component {
    async fn summarize(transcription: String) -> StreamReader<u8> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("info")
            .with_writer(std::io::stderr)
            .try_init();

        info!("simd128: {}", candle::utils::with_simd128(),);
        
        info!("Summarize transcription backend started");

        let (mut tx, rx) = bindings::wit_stream::new::<u8>();

        wit_bindgen::spawn_local(async move {
            
            let model_path = resolve_data_path(MODEL_RELATIVE_PATH);
            let mut file = std::fs::File::open(&model_path).unwrap_or_else(|e| {
                panic!("LLM Model file should exist and be valid: {model_path:?}: {e}")
            });
            let start = std::time::Instant::now();

            info!("AI Model: Qwen3-0.6B-Q4_K_M.gguf");

            let cpu = true;
            let device = device(cpu).unwrap();

            info!("CPU Backend to run AI workload.");

            let mut model = {
                let model = gguf_file::Content::read(&mut file)
                    .map_err(|e| e.with_path(model_path))
                    .unwrap();
                let mut total_size_in_bytes = 0;
                for (_, tensor) in model.tensor_infos.iter() {
                    let elem_count = tensor.shape.elem_count();
                    total_size_in_bytes +=
                        elem_count * tensor.ggml_dtype.type_size() / tensor.ggml_dtype.block_size();
                }
                info!(
                    "loaded {:?} tensors ({}) in {:.2}s",
                    model.tensor_infos.len(),
                    total_size_in_bytes.to_string(),
                    start.elapsed().as_secs_f32(),
                );
                Qwen3::from_gguf(model, &mut file, &device).unwrap()
            };

            info!("AI Model: Qwen3-0.6B-Q4_K_M.gguf");
            info!("AI Model built");

            let tokenizer_path = resolve_data_path(TOKENIZER_RELATIVE_PATH);

            let tokenizer = Tokenizer::from_file(&tokenizer_path).unwrap_or_else(|e| {
                panic!("Tokenizer file should exist and be valid: {tokenizer_path:?}: {e}")
            });

            let mut tos = TokenOutputStream::new(tokenizer);

            info!(transcription = transcription, "Transcription received.");

            let prompt_str = match configure_prompts(transcription, "/no_think".to_string()) {
                Ok(p) => p,
                Err(e) => {
                    info!("Prompt processing failed.");

                    error!(
                        "Failed to configure prompts: data/user-prompt.txt and data/system-prompt.txt {:?}",
                        e
                    );

                    let err_msg = format!("Error: {}", e);
                    let _ = tx.write_all(err_msg.into_bytes()).await;
                    // Close the stream (sender drops, receiver gets None)
                    drop(tx);
                    return; // exit the spawn block early
                }
            };

            let tokens = tos.tokenizer().encode(prompt_str, true).unwrap();

            let tokens = tokens.get_ids();
            let mut all_tokens = vec![];
            
            let sample_len: usize = 1000;
            let to_sample = sample_len.saturating_sub(1);
            let temperature: f64 = 0.6;
            let top_p: Option<f64> = Some(0.95);
            let top_k: Option<usize> = Some(20);
            let seed: u64 = 0;
            let repeat_penalty: f32 = 1.5;
            let repeat_last_n: usize = 64;
            let eos_token = tos.tokenizer().token_to_id("<|im_end|>").unwrap();
            
            let mut logits_processor = {
                let temperature = temperature;
                let sampling = if temperature <= 0. {
                    Sampling::ArgMax
                } else {
                    match (top_k, top_p) {
                        (None, None) => Sampling::All { temperature },
                        (Some(k), None) => Sampling::TopK { k, temperature },
                        (None, Some(p)) => Sampling::TopP { p, temperature },
                        (Some(k), Some(p)) => Sampling::TopKThenTopP { k, p, temperature },
                    }
                };
                LogitsProcessor::from_sampling(seed, sampling)
            };

            let input = Tensor::new(tokens, &device).unwrap().unsqueeze(0).unwrap();
            let logits = model.forward(&input, 0).unwrap();
            let logits = logits.squeeze(0).unwrap();
            let mut next_token = logits_processor.sample(&logits).unwrap();

            all_tokens.push(next_token);

            info!("Prompt processing started.");

            if let Some(t) = tos.next_token(next_token).unwrap() {
                info!(t);

                tx.write_all(t.into_bytes()).await;
            }

            for index in 0..to_sample {
                let input = Tensor::new(&[next_token], &device)
                    .unwrap()
                    .unsqueeze(0)
                    .unwrap();
                let logits = model.forward(&input, tokens.len() + index).unwrap();
                let logits = logits.squeeze(0).unwrap();
                let logits = if repeat_penalty == 1. {
                    logits
                } else {
                    let start_at = all_tokens.len().saturating_sub(repeat_last_n);
                    candle_transformers::utils::apply_repeat_penalty(
                        &logits,
                        repeat_penalty,
                        &all_tokens[start_at..],
                    )
                    .unwrap()
                };
                next_token = logits_processor.sample(&logits).unwrap();
                all_tokens.push(next_token);
                if let Some(t) = tos.next_token(next_token).unwrap() {
                    info!("{t}");
                    tx.write_all(t.into_bytes()).await;
                }

                if next_token == eos_token {
                    break;
                };
            }

            if let Some(rest) = tos.decode_rest().map_err(candle::Error::msg).unwrap() {
                info!("{rest}");
                tx.write_all(rest.into_bytes()).await;
            }

            info!("Prompt processing end.");
            info!("Summarize transcription complete.");
            info!("*****************************************");

            drop(tx);
        });
        rx
    }
}

bindings::export!(Component with_types_in bindings);

#[cfg(test)]
mod tests {
    use super::{MODEL_RELATIVE_PATH, TOKENIZER_RELATIVE_PATH, resolve_data_path};

    #[test]
    fn resolves_model_assets_from_testdata() {
        let model_path = resolve_data_path(MODEL_RELATIVE_PATH);
        let tokenizer_path = resolve_data_path(TOKENIZER_RELATIVE_PATH);

        assert!(model_path.ends_with(MODEL_RELATIVE_PATH));
        assert!(tokenizer_path.ends_with(TOKENIZER_RELATIVE_PATH));
    }
}
