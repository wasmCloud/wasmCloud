# Wasi AI App

A WasmCloud AI application that transcribes audio files using a ggml-tiny.bin model and summarizes the resulting text with a lightweight Qwen model, all orchestrated through a web UI. Built with Rust, WASI Components, and the WasmCloud.
App uses a local LLM to generate meeting notes.

# Components
1. Transcriber : Generate transcription for the Audio file
2. Summarizer : Summarize transcription 
3. Web : UI


✨ 🚀 Start => 🎧 Transcriber => 📝 Summarizer => Done 🏁

# Transcriber Component
- Generate transcription for the Audio file
- Audio file as Input: output.wav | Size: 141.3 MB
- AI Model to generate transcription: [ggml-tiny.bin](https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-tiny.bin) | Size: 77.7 MB

- Srack: Rust, oxiwhisper, Wasi, WasmCloud

# Summarizer Component
- Summarize transcription
- Audio file transcription as Input: Text
- AI Model to Summarize transcription: [Qwen3-0.6B-Q4_K_M.gguf](https://huggingface.co/Qwen/Qwen3-0.6B-GGUF) | Size: 396.7 MB

- Srack: Rust, Candle, Wasi, WasmCloud

# Web Component
- Web UI
- Workflow, Run, Status, Description

- Srack: Rust, JS, CSS, HTML, Wasi, WasmCloud

# The files required in testdata dir

- testdata/Qwen3-0.6B-Q4_K_M/Qwen3-0.6B-Q4_K_M.gguf
- testdata/Qwen3-0.6B-Q4_K_M/tokenizer.json
- testdata/ggml-tiny.bin
- testdata/sample_audio.wav
- testdata/user-prompt.txt
- testdata/system-prompt.txt


# Check the local k8s deployment guide 
- deployment/readme.md
- deployment/deployment.yaml
-  [Deploy a Wasm Workload to Kubernetes](https://wasmcloud.com/docs/quickstart/deploy-a-webassembly-workload/)

![Built with](https://cdn.prod.website-files.com/68e941b736876afc9468db2b/68e941b736876afc9468dcda_Wasmcloud.Logo-Hrztl_Color.png)
