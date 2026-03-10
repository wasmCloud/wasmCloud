fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(false)
        .build_server(true)
        .build_transport(false) // Don't generate transport code for WASI
        .compile_protos(&["../proto/helloworld.proto"], &["../proto"])?;
    Ok(())
}
