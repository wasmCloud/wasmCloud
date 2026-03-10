fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .build_transport(false) // Don't generate transport code for WASI
        .compile_protos(&["../proto/helloworld.proto"], &["../proto"])?;
    Ok(())
}
