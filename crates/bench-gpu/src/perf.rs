//! What a llama.cpp tool's stderr says about where a run's time went.

use std::{
    io::Write as _,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
};

use wasmtime_wasi::cli::{IsTerminal, StdoutStream};

/// Bytes of stderr kept per run. llama.cpp prints its timings last.
const TAIL: usize = 64 << 10;

/// Times from `llama_perf_context_print`, in milliseconds.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Timings {
    /// Model load: reading the weights into device buffers.
    pub load_ms: Option<f64>,
    /// Prompt eval plus token eval: the first token, for a `-n 1` run.
    pub first_token_ms: Option<f64>,
}

impl Timings {
    pub fn parse(stderr: &str) -> Self {
        let mut load = None;
        let mut prompt = None;
        let mut eval = None;
        for line in stderr.lines() {
            let Some(rest) = line.split_once("llama_perf_context_print:").map(|(_, r)| r) else {
                continue;
            };
            let Some((label, value)) = rest.split_once('=') else {
                continue;
            };
            let ms = value
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok());
            match label.trim() {
                "load time" => load = ms,
                "prompt eval time" => prompt = ms,
                "eval time" => eval = ms,
                _ => {}
            }
        }
        Self {
            load_ms: load,
            first_token_ms: prompt.map(|p| p + eval.unwrap_or(0.0)),
        }
    }
}

/// Passes a guest's stderr through to this process's, keeping the tail.
#[derive(Clone, Default)]
pub struct Tee(Arc<Mutex<Vec<u8>>>);

impl Tee {
    pub fn record(&self, bytes: &[u8]) {
        let _ = std::io::stderr().write_all(bytes);
        let mut tail = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        tail.extend_from_slice(bytes);
        if tail.len() > TAIL {
            let cut = tail.len() - TAIL;
            tail.drain(..cut);
        }
    }

    pub fn text(&self) -> String {
        let tail = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&tail).into_owned()
    }
}

impl IsTerminal for Tee {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for Tee {
    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

impl tokio::io::AsyncWrite for Tee {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.record(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERF: &str = "\
main: decoded 1 tokens in 0.05 s, speed: 20.00 t/s
llama_perf_sampler_print:    sampling time =       0.10 ms /     2 runs   (    0.05 ms per token, 20000.00 tokens per second)
llama_perf_context_print:        load time =    1234.50 ms
llama_perf_context_print: prompt eval time =      40.25 ms /     2 tokens (   20.12 ms per token,    49.69 tokens per second)
llama_perf_context_print:        eval time =       9.75 ms /     1 runs   (    9.75 ms per token,   102.56 tokens per second)
llama_perf_context_print:       total time =    1300.00 ms /     3 tokens
";

    #[test]
    fn parses_load_and_first_token() {
        let t = Timings::parse(PERF);
        assert_eq!(t.load_ms, Some(1234.5));
        assert_eq!(t.first_token_ms, Some(50.0));
    }

    #[test]
    fn missing_timings_are_none() {
        assert_eq!(
            Timings::parse("error: unable to load model\n"),
            Timings::default()
        );
    }

    #[test]
    fn tee_keeps_only_the_tail() {
        let tee = Tee::default();
        tee.record(&vec![b'a'; TAIL]);
        tee.record(b"end");
        let text = tee.text();
        assert_eq!(text.len(), TAIL);
        assert!(text.ends_with("aend"));
    }
}
