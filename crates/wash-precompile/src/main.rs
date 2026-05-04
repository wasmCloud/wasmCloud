use clap::Parser;

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

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();
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
