use clap::Parser;
use url::Url;

mod output;
mod precompile;
mod pull;

#[derive(Parser, Debug)]
#[command(name = "wash-precompile")]
struct Args {
    /// OCI reference to the source component
    #[arg(long)]
    image: String,

    /// URL where the precompiled .cwasm bytes will be written
    #[arg(long)]
    output: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let output_url = Url::parse(&args.output)?;

    let wasm = pull::fetch(&args.image).await?;
    let cwasm = precompile::compile(&wasm)?;
    output::write(&output_url, &cwasm)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_image_and_output_flags() {
        let args = Args::parse_from([
            "wash-precompile",
            "--image",
            "ghcr.io/example/comp:v1",
            "--output",
            "file:///tmp/out.cwasm",
        ]);
        assert_eq!(args.image, "ghcr.io/example/comp:v1");
        assert_eq!(args.output, "file:///tmp/out.cwasm");
    }
}
