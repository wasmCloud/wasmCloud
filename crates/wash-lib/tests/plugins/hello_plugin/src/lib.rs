#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!();

use std::io::Read;

use exports::wasi::cli::run::Guest as RunGuest;
use exports::wasmcloud::wash::subcommand::{Guest as SubcommandGuest, Metadata};
use wasi::cli::environment;
use wasi::http::types::*;

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
    status: String,
}

struct HelloPlugin;

impl RunGuest for HelloPlugin {
    fn run() -> Result<(), ()> {
        println!("I got some arguments: {:?}", environment::get_arguments());
        println!(
            "I got some environment variables: {:?}",
            environment::get_environment()
        );
        let req = wasi::http::outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Https))?;
        req.set_authority(Some("dog.ceo"))?;
        req.set_path_with_query(Some("/api/breeds/image/random"))?;
        match wasi::http::outgoing_handler::handle(req, None) {
            Ok(resp) if wasi::io::poll::poll(&[&resp.subscribe()]) == [0] => {
                let response = resp
                    .get()
                    .expect("HTTP request response missing")
                    .expect("HTTP request response requested more than once")
                    .expect("HTTP request failed");
                if response.status() == 200 {
                    let response_body = response
                        .consume()
                        .expect("failed to get incoming request body");
                    let body = {
                        let mut buf = vec![];
                        let mut stream = response_body
                            .stream()
                            .expect("failed to get HTTP request response stream");
                        InputStreamReader::from(&mut stream)
                            .read_to_end(&mut buf)
                            .expect("failed to read value from HTTP request response stream");
                        buf
                    };
                    let _trailers = wasi::http::types::IncomingBody::finish(response_body);
                    let dog_response: DogResponse = match serde_json::from_slice(&body) {
                        Ok(d) => d,
                        Err(e) => {
                            println!("Failed to deserialize dog response: {}", e);
                            DogResponse {
                                message: "Failed to deserialize dog response".to_string(),
                                status: "failure".to_string(),
                            }
                        }
                    };
                    println!(
                        "{}! Here have a dog picture: {}",
                        dog_response.status, dog_response.message
                    );
                } else {
                    println!("HTTP request failed with status code {}", response.status());
                }
            }
            Ok(_) => {
                println!("Got response, but it wasn't ready");
            }
            Err(e) => {
                println!("Got error when trying to fetch dog: {}", e);
            }
        }
        println!("Hello from the plugin");
        Ok(())
    }
}

impl SubcommandGuest for HelloPlugin {
    fn register() -> Metadata {
        Metadata {
            name: "Hello Plugin".to_string(),
            id: "hello".to_string(),
            description: "A simple plugin that says hello and logs a bunch of things".to_string(),
            author: "WasmCloud".to_string(),
            version: "0.1.0".to_string(),
            flags: vec![("--foo".to_string(), "A foo variable".to_string())],
            arguments: vec![("name".to_string(), "A random name".to_string())],
        }
    }
}

pub struct InputStreamReader<'a> {
    stream: &'a mut crate::wasi::io::streams::InputStream,
}

impl<'a> From<&'a mut crate::wasi::io::streams::InputStream> for InputStreamReader<'a> {
    fn from(stream: &'a mut crate::wasi::io::streams::InputStream) -> Self {
        Self { stream }
    }
}

impl std::io::Read for InputStreamReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use crate::wasi::io::streams::StreamError;
        use std::io;

        let n = buf
            .len()
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        match self.stream.blocking_read(n) {
            Ok(chunk) => {
                let n = chunk.len();
                if n > buf.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "more bytes read than requested",
                    ));
                }
                buf[..n].copy_from_slice(&chunk);
                Ok(n)
            }
            Err(StreamError::Closed) => Ok(0),
            Err(StreamError::LastOperationFailed(e)) => {
                Err(io::Error::new(io::ErrorKind::Other, e.to_debug_string()))
            }
        }
    }
}

export!(HelloPlugin);
