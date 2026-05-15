fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_protos(&["../proto/helloworld.proto"], &["../proto"])?;
    Ok(())
}
