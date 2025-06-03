wit_bindgen::generate!({ generate_all });

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::builder::ValueParser;
use clap::{Arg, CommandFactory, FromArgMatches};
use exports::wasi::cli::run::Guest as RunGuest;
use exports::wasmcloud::wash::subcommand::{Argument, Guest as SubcommandGuest, Metadata};
use wasi::cli::environment;
use wasi::filesystem::preopens::get_directories;
use wasi::filesystem::types::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};
use wasi::http::types::*;

impl From<&Arg> for Argument {
    fn from(arg: &Arg) -> Self {
        Self {
            description: arg.get_help().map(ToString::to_string).unwrap_or_default(),
            is_path: arg.get_value_parser().type_id() == ValueParser::path_buf().type_id(),
            required: arg.is_required_set(),
        }
    }
}

#[derive(clap::Parser)]
#[clap(name = "hello")]
struct Hello {
    /// A random string
    #[clap(long = "bar")]
    bar: Option<String>,

    /// A directory to read
    #[clap(long = "foo")]
    foo: PathBuf,

    /// A file to read
    #[clap(id = "path")]
    path: PathBuf,
}

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
    status: String,
}

struct HelloPlugin;

// Our implementation of the wasi:cli/run interface
impl RunGuest for HelloPlugin {
    fn run() -> Result<(), ()> {
        // An example for reading command line arguments and environment variables
        let args = environment::get_arguments();
        println!("I got some arguments: {:?}", args);
        println!(
            "I got some environment variables: {:?}",
            environment::get_environment()
        );

        let cmd = Hello::command();
        let matches = match cmd.try_get_matches_from(args) {
            Ok(matches) => matches,
            Err(err) => {
                eprintln!("Error parsing arguments: {}", err);
                return Err(());
            }
        };
        let args = match Hello::from_arg_matches(&matches) {
            Ok(args) => args,
            Err(err) => {
                eprintln!("Error parsing arguments: {}", err);
                return Err(());
            }
        };

        // An example of an outgoing HTTP request. Hopefully we'll have a helper crate to make this
        // easier soon!
        let req = wasi::http::outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Https))?;
        req.set_authority(Some("dog.ceo"))?;
        req.set_path_with_query(Some("/api/breeds/image/random"))?;
        match wasi::http::outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();
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
                    eprintln!("HTTP request failed with status code {}", response.status());
                }
            }
            Err(e) => {
                eprintln!("Got error when trying to fetch dog: {}", e);
            }
        }

        // An example of writing to a file. It will be very similar to read from a file as well if
        // you want to load a config file. To open a file, you have to get the directory that was
        // given to the plugin.
        if let Ok(dir) = get_dir("/") {
            let file = dir
                .open_at(
                    PathFlags::empty(),
                    "hello.txt",
                    OpenFlags::CREATE,
                    DescriptorFlags::READ | DescriptorFlags::WRITE,
                )
                .expect("Should be able to access file");
            file.write(b"Hello from the plugin", 0)
                .expect("Should be able to write to file");
        }

        if let Ok(dir) = get_dir(&args.foo) {
            let entries = dir.read_directory().map_err(|e| {
                eprintln!("Failed to read directory: {}", e);
            })?;
            println!("Directory entries for {}:", args.foo.display());
            while let Some(res) = entries.read_directory_entry().transpose() {
                let entry = res.map_err(|e| {
                    eprintln!("Failed to read directory entry: {}", e);
                })?;
                println!("{}", entry.name);
            }
        }

        let file =
            open_file(&args.path, OpenFlags::empty(), DescriptorFlags::READ).map_err(|e| {
                eprintln!("Failed to open file: {}", e);
            })?;

        let mut body = file.read_via_stream(0).map_err(|e| {
            eprintln!("Failed to read file: {}", e);
        })?;
        let mut buf = vec![];
        InputStreamReader::from(&mut body)
            .read_to_end(&mut buf)
            .map_err(|e| {
                eprintln!("Failed to read file: {}", e);
            })?;
        println!(
            "The file {} has the contents {}",
            args.path.display(),
            String::from_utf8_lossy(&buf)
        );

        println!("Hello from the plugin");
        Ok(())
    }
}

// Our plugin's metadata implemented for the subcommand interface
impl SubcommandGuest for HelloPlugin {
    fn register() -> Metadata {
        let cmd = Hello::command();
        let (arguments, flags): (Vec<_>, Vec<_>) =
            cmd.get_arguments().partition(|arg| arg.is_positional());
        // There isn't a partition_map function without importing another crate
        let arguments = arguments
            .into_iter()
            .map(|arg| (arg.get_id().to_string(), Argument::from(arg)))
            .collect();
        let flags = flags
            .into_iter()
            .map(|arg| (arg.get_id().to_string(), Argument::from(arg)))
            .collect();
        Metadata {
            name: "Hello Plugin".to_string(),
            id: "hello".to_string(),
            description: "A simple plugin that says hello and logs a bunch of things".to_string(),
            author: "WasmCloud".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            flags,
            arguments,
        }
    }
}

fn get_dir(path: impl AsRef<Path>) -> Result<Descriptor, String> {
    get_directories()
        .into_iter()
        .find_map(|(dir, dir_path)| {
            (<std::string::String as std::convert::AsRef<Path>>::as_ref(&dir_path) == path.as_ref())
                .then_some(dir)
        })
        .ok_or_else(|| format!("Could not find directory {}", path.as_ref().display()))
}

/// Opens the given file. This should be the canonicalized path to the file.
fn open_file(
    path: impl AsRef<Path>,
    open_flags: OpenFlags,
    descriptor_flags: DescriptorFlags,
) -> Result<Descriptor, String> {
    let dir = path
        .as_ref()
        .parent()
        // I mean, if someone passed a path that is at the root, that probably wasn't a good idea
        .ok_or_else(|| {
            format!(
                "Could not find parent directory of {}",
                path.as_ref().display()
            )
        })?;
    let dir = get_dir(dir)?;
    dir.open_at(
        PathFlags::empty(),
        path.as_ref()
            .file_name()
            .ok_or_else(|| format!("Path did not have a file name: {}", path.as_ref().display()))?
            .to_str()
            .ok_or_else(|| "Path is not a valid string".to_string())?,
        open_flags,
        descriptor_flags,
    )
    .map_err(|e| format!("Failed to open file {}: {}", path.as_ref().display(), e))
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
