use anyhow::Result;
use url::Url;

pub fn write(output: &Url, bytes: &[u8]) -> Result<()> {
    match output.scheme() {
        "file" => {
            let path = output
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("invalid file:// URL: {output}"))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            std::fs::write(&path, bytes)?;
            Ok(())
        }
        other => anyhow::bail!("unsupported output scheme: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bytes_to_file_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.cwasm");
        let url = Url::from_file_path(&path).unwrap();

        write(&url, b"hello").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn unknown_scheme_errors() {
        let url = Url::parse("s3://bucket/key").unwrap();
        let err = write(&url, b"x").unwrap_err();
        assert!(err.to_string().contains("unsupported output scheme"));
    }
}
