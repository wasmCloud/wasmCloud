use crate::Result;
use async_compression::{
    tokio::{bufread::GzipDecoder, write::GzipEncoder},
    Level,
};
use data_encoding::HEXUPPER;
use ring::digest::{Context, Digest, SHA256};
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufReader},
};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
use wascap::{
    jwt::{CapabilityProvider, Claims},
    prelude::KeyPair,
};

const CLAIMS_JWT_FILE: &str = "claims.jwt";

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// A provider archive is a specialized ZIP file that contains a set of embedded and signed claims
/// (a .JWT file) as well as a list of binary files, one plugin library for each supported
/// target architecture and OS combination
pub struct ProviderArchive {
    libraries: HashMap<String, Vec<u8>>,
    capid: String,
    name: String,
    vendor: String,
    rev: Option<i32>,
    ver: Option<String>,
    claims: Option<Claims<CapabilityProvider>>,
    json_schema: Option<serde_json::Value>,
}

impl ProviderArchive {
    /// Creates a new provider archive in memory, to which native library files can be added.
    pub fn new(
        capid: &str,
        name: &str,
        vendor: &str,
        rev: Option<i32>,
        ver: Option<String>,
    ) -> ProviderArchive {
        ProviderArchive {
            libraries: HashMap::new(),
            capid: capid.to_string(),
            name: name.to_string(),
            vendor: vendor.to_string(),
            rev,
            ver,
            claims: None,
            json_schema: None,
        }
    }

    /// Adds a native library file (.so, .dylib, .dll) to the archive for a given target string
    pub fn add_library(&mut self, target: &str, input: &[u8]) -> Result<()> {
        self.libraries.insert(target.to_string(), input.to_vec());

        Ok(())
    }

    /// Sets a JSON schema for this provider's link definition specification. This will be injected
    /// into the claims written to a provider's PAR file, so you'll need to do this after instantiation
    /// and prior to writing
    pub fn set_schema(&mut self, schema: serde_json::Value) -> Result<()> {
        self.json_schema = Some(schema);

        Ok(())
    }

    /// Gets the list of architecture/OS targets within the archive
    pub fn targets(&self) -> Vec<String> {
        self.libraries.keys().cloned().collect()
    }

    /// Retrieves the raw bytes for a given target
    pub fn target_bytes(&self, target: &str) -> Option<Vec<u8>> {
        self.libraries.get(target).cloned()
    }

    /// Returns the embedded claims associated with this archive. Note that claims are not available
    /// while building a new archive. They are only available after the archive has been written
    /// or if the archive was loaded from an existing file
    pub fn claims(&self) -> Option<Claims<CapabilityProvider>> {
        self.claims.clone()
    }

    /// Obtains the JSON schema if one was either set explicitly on the structure or loaded from
    /// claims in the PAR
    pub fn schema(&self) -> Option<serde_json::Value> {
        self.json_schema.clone()
    }

    /// Attempts to read a Provider Archive (PAR) file's bytes to analyze and verify its contents.
    ///
    /// The embedded claims in this archive will be validated, and the file hashes contained in
    /// those claims will be compared and verified against hashes computed at load time. This
    /// prevents the contents of the archive from being modified without the embedded claims being
    /// re-signed. This will load all binaries into memory in the returned `ProviderArchive`.
    ///
    /// Please note that this method requires that you have _all_ of the provider archive bytes in
    /// memory, which will likely be really hefty if you are just trying to load a specific binary
    /// to run
    pub async fn try_load(input: &[u8]) -> Result<ProviderArchive> {
        let mut cursor = Cursor::new(input);
        Self::load(&mut cursor, None).await
    }

    /// Attempts to read a Provider Archive (PAR) file's bytes to analyze and verify its contents,
    /// loading _only_ the specified target.
    ///
    /// This is useful when loading a provider archive for consumption and you know the target OS
    /// you need. The embedded claims in this archive will be validated, and the file hashes
    /// contained in those claims will be compared and verified against hashes computed at load
    /// time. This prevents the contents of the archive from being modified without the embedded
    /// claims being re-signed
    ///
    /// Please note that this method requires that you have _all_ of the provider archive bytes in
    /// memory, which will likely be really hefty if you are just trying to load a specific binary
    /// to run
    pub async fn try_load_target(input: &[u8], target: &str) -> Result<ProviderArchive> {
        let mut cursor = Cursor::new(input);
        Self::load(&mut cursor, Some(target)).await
    }

    /// Attempts to read a Provider Archive (PAR) file to analyze and verify its contents.
    ///
    /// The embedded claims in this archive will be validated, and the file hashes contained in
    /// those claims will be compared and verified against hashes computed at load time. This
    /// prevents the contents of the archive from being modified without the embedded claims being
    /// re-signed. This will load all binaries into memory in the returned `ProviderArchive`. Use
    /// [`load`] or [`try_load_target_from_file`]  methods if you only want to load a single binary
    /// into memory.
    pub async fn try_load_file(path: impl AsRef<Path>) -> Result<ProviderArchive> {
        let mut file = File::open(path).await?;
        Self::load(&mut file, None).await
    }

    /// Attempts to read a Provider Archive (PAR) file to analyze and verify its contents.
    ///
    /// The embedded claims in this archive will be validated, and the file hashes contained in
    /// those claims will be compared and verified against hashes computed at load time. This
    /// prevents the contents of the archive from being modified without the embedded claims being
    /// re-signed. This will only read a single binary into memory.
    ///
    /// It is recommended to use this method or the [`load`] method when consuming a provider
    /// archive. Otherwise all binaries will be loaded into memory
    pub async fn try_load_target_from_file(
        path: impl AsRef<Path>,
        target: &str,
    ) -> Result<ProviderArchive> {
        let mut file = File::open(path).await?;
        Self::load(&mut file, Some(target)).await
    }

    /// Attempts to read a Provider Archive (PAR) from a Reader to analyze and verify its contents.
    /// The optional `target` parameter allows you to select a single binary to load
    ///
    /// The embedded claims in this archive will be validated, and the file hashes contained in
    /// those claims will be compared and verified against hashes computed at load time. This
    /// prevents the contents of the archive from being modified without the embedded claims being
    /// re-signed. If a `target` is specified, this will only read a single binary into memory.
    ///
    /// This is the most generic loading option available and allows you to load from anything that
    /// implements `AsyncRead` and `AsyncSeek`
    pub async fn load<R: AsyncRead + AsyncSeek + Unpin + Send + Sync>(
        input: &mut R,
        target: Option<&str>,
    ) -> Result<ProviderArchive> {
        let mut libraries = HashMap::new();

        let mut magic = [0; 2];
        if let Err(e) = input.read_exact(&mut magic).await {
            // If we can't fill the buffer, it isn't a valid par file
            if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof) {
                return Err("Not enough bytes to be a valid PAR file".into());
            }
            return Err(e.into());
        }

        // Seek back to beginning
        input.rewind().await?;

        let mut par = Archive::new(if magic == GZIP_MAGIC {
            Box::new(GzipDecoder::new(BufReader::new(input)))
                as Box<dyn AsyncRead + Unpin + Sync + Send>
        } else {
            Box::new(input) as Box<dyn AsyncRead + Unpin + Sync + Send>
        });

        let mut c: Option<Claims<CapabilityProvider>> = None;

        let mut entries = par.entries()?;

        while let Some(res) = entries.next().await {
            let mut entry = res?;
            let mut bytes = Vec::new();
            let file_target = PathBuf::from(entry.path()?)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            if file_target == "claims" {
                tokio::io::copy(&mut entry, &mut bytes).await?;
                c = Some(Claims::<CapabilityProvider>::decode(std::str::from_utf8(
                    &bytes,
                )?)?);
            } else if let Some(t) = target {
                // If loading only a specific target, only copy in bytes if it is the target. We still
                // need to iterate through the rest so we can be sure to find the claims
                if file_target == t {
                    tokio::io::copy(&mut entry, &mut bytes).await?;
                    libraries.insert(file_target.to_string(), bytes);
                }
                continue;
            } else {
                tokio::io::copy(&mut entry, &mut bytes).await?;
                libraries.insert(file_target.to_string(), bytes);
            }
        }

        if c.is_none() || libraries.is_empty() {
            // we need at least claims.jwt and one plugin binary
            libraries.clear();
            return Err(
                "Not enough files found in provider archive. Is this a complete archive?".into(),
            );
        }

        if let Some(ref cl) = c {
            let metadata = cl.metadata.as_ref().unwrap();
            let name = cl.name();
            let capid = metadata.capid.to_string();
            let vendor = metadata.vendor.to_string();
            let rev = metadata.rev;
            let ver = metadata.ver.clone();
            let json_schema = metadata.config_schema.clone();

            validate_hashes(&libraries, c.as_ref().unwrap())?;

            Ok(ProviderArchive {
                libraries,
                capid,
                name,
                vendor,
                rev,
                ver,
                claims: c,
                json_schema,
            })
        } else {
            Err("No claims found embedded in provider archive.".into())
        }
    }

    /// Generates a Provider Archive (PAR) file with all of the library files and a signed set of claims in an embedded JWT
    pub async fn write(
        &mut self,
        destination: impl AsRef<Path>,
        issuer: &KeyPair,
        subject: &KeyPair,
        compress_par: bool,
    ) -> Result<()> {
        let file = File::create(
            if compress_par && destination.as_ref().extension().unwrap_or_default() != "gz" {
                let mut file_name = destination
                    .as_ref()
                    .file_name()
                    .ok_or("Destination is not a file")?
                    .to_owned();
                file_name.push(".gz");
                destination.as_ref().with_file_name(file_name)
            } else {
                destination.as_ref().to_owned()
            },
        )
        .await?;

        let mut par = tokio_tar::Builder::new(if compress_par {
            Box::new(GzipEncoder::with_quality(file, Level::Best))
                as Box<dyn AsyncWrite + Send + Sync + Unpin>
        } else {
            Box::new(file) as Box<dyn AsyncWrite + Send + Sync + Unpin>
        });

        let mut claims = Claims::<CapabilityProvider>::new(
            self.name.to_string(),
            issuer.public_key(),
            subject.public_key(),
            self.capid.to_string(),
            self.vendor.to_string(),
            self.rev,
            self.ver.clone(),
            generate_hashes(&self.libraries),
        );
        if let Some(schema) = self.json_schema.clone() {
            claims.metadata.as_mut().unwrap().config_schema = Some(schema);
        }
        self.claims = Some(claims.clone());

        let claims_file = claims.encode(issuer)?;

        let mut header = tokio_tar::Header::new_gnu();
        header.set_path(CLAIMS_JWT_FILE)?;
        header.set_size(claims_file.as_bytes().len() as u64);
        header.set_cksum();
        par.append_data(&mut header, CLAIMS_JWT_FILE, Cursor::new(claims_file))
            .await?;

        for (tgt, lib) in self.libraries.iter() {
            let mut header = tokio_tar::Header::new_gnu();
            let path = format!("{}.bin", tgt);
            header.set_path(&path)?;
            header.set_size(lib.len() as u64);
            header.set_cksum();
            par.append_data(&mut header, &path, Cursor::new(lib))
                .await?;
        }

        // Completes the process of packing a .par archive
        let mut inner = par.into_inner().await?;
        // Make sure everything is flushed to disk, otherwise we might miss closing data block
        inner.flush().await?;
        inner.shutdown().await?;

        Ok(())
    }
}

fn validate_hashes(
    libraries: &HashMap<String, Vec<u8>>,
    claims: &Claims<CapabilityProvider>,
) -> Result<()> {
    let file_hashes = claims.metadata.as_ref().unwrap().target_hashes.clone();

    for (tgt, library) in libraries.iter() {
        let file_hash = file_hashes.get(tgt).cloned().unwrap();
        let check_hash = hash_bytes(library);
        if file_hash != check_hash {
            return Err(format!("File hash and verify hash do not match for '{}'", tgt).into());
        }
    }
    Ok(())
}

fn generate_hashes(libraries: &HashMap<String, Vec<u8>>) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    for (target, lib) in libraries.iter() {
        let hash = hash_bytes(lib);
        hm.insert(target.to_string(), hash);
    }

    hm
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = sha256_digest(bytes).unwrap();
    HEXUPPER.encode(digest.as_ref())
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<Digest> {
    let mut context = Context::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context.finish())
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;
    use wascap::prelude::KeyPair;

    #[tokio::test]
    async fn write_par() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(1),
            Some("0.0.1".to_string()),
        );
        arch.add_library("aarch64-linux", b"blahblah")?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let outpath = tempdir.path().join("writetest.par");
        arch.write(&outpath, &issuer, &subject, false).await?;
        tokio::fs::metadata(outpath)
            .await
            .expect("Unable to locate newly created par file");

        Ok(())
    }

    #[tokio::test]
    async fn error_on_no_providers() -> Result<()> {
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(2),
            Some("0.0.2".to_string()),
        );

        let tempdir = tempfile::tempdir()?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let outpath = tempdir.path().join("shoulderr.par");
        arch.write(&outpath, &issuer, &subject, false).await?;

        let mut buf2 = Vec::new();
        let mut f2 = File::open(outpath).await?;
        f2.read_to_end(&mut buf2).await?;

        let arch2 = ProviderArchive::try_load(&buf2).await;

        match arch2 {
            Ok(_notok) => panic!("Loading an archive without any libraries should fail"),
            Err(_e) => (),
        }

        Ok(())
    }

    #[tokio::test]
    async fn round_trip() -> Result<()> {
        // Build an archive in memory the way a CLI wrapper might...
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(3),
            Some("0.0.3".to_string()),
        );
        arch.add_library("aarch64-linux", b"blahblah")?;
        arch.add_library("x86_64-linux", b"bloobloo")?;
        arch.add_library("x86_64-macos", b"blarblar")?;
        arch.set_schema(json!({"property":"foo"}))?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let tempdir = tempfile::tempdir()?;

        let firstpath = tempdir.path().join("firstarchive.par");
        let secondpath = tempdir.path().join("secondarchive.par");

        // Generate the .par file with embedded claims.jwt file (needs a service and an account key)
        arch.write(&firstpath, &issuer, &subject, false).await?;

        // Try loading from file
        let arch2 = ProviderArchive::try_load_file(&firstpath).await?;
        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries.get("aarch64-linux"),
            arch2.libraries.get("aarch64-linux")
        );
        assert_eq!(
            arch.libraries.get("x86_64-macos"),
            arch2.libraries.get("x86_64-macos")
        );
        assert_eq!(arch.claims().unwrap().subject, subject.public_key());

        // Load just one of the binaries
        let arch2 = ProviderArchive::try_load_target_from_file(&firstpath, "aarch64-linux").await?;
        assert_eq!(
            arch.libraries.get("aarch64-linux"),
            arch2.libraries.get("aarch64-linux")
        );
        assert!(
            arch2.libraries.get("x86_64-macos").is_none(),
            "Should have loaded only one binary"
        );
        assert_eq!(
            arch2.claims().unwrap().subject,
            subject.public_key(),
            "Claims should still load"
        );

        let json = arch2
            .claims()
            .unwrap()
            .metadata
            .unwrap()
            .config_schema
            .unwrap();
        assert_eq!(json, json!({"property":"foo"}));

        let mut buf2 = Vec::new();
        let mut f2 = File::open(&firstpath).await?;
        f2.read_to_end(&mut buf2).await?;

        // Make sure the file we wrote can be read back in with no data loss
        let mut arch2 = ProviderArchive::try_load(&buf2).await?;
        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries.get("aarch64-linux"),
            arch2.libraries.get("aarch64-linux")
        );
        assert_eq!(arch.claims().unwrap().subject, subject.public_key());

        // Another common task - read an existing archive and add another library file to it
        arch2.add_library("mips-linux", b"bluhbluh")?;
        arch2.write(&secondpath, &issuer, &subject, false).await?;

        let mut buf3 = Vec::new();
        let mut f3 = File::open(&secondpath).await?;
        f3.read_to_end(&mut buf3).await?;

        // Make sure the re-written/modified archive looks the way we expect
        let arch3 = ProviderArchive::try_load(&buf3).await?;
        assert_eq!(arch3.capid, arch2.capid);
        assert_eq!(
            arch3.libraries[&"aarch64-linux".to_string()],
            arch2.libraries[&"aarch64-linux".to_string()]
        );
        assert_eq!(arch3.claims().unwrap().subject, subject.public_key());
        assert_eq!(arch3.targets().len(), 4);

        Ok(())
    }

    #[tokio::test]
    async fn compression_roundtrip() -> Result<()> {
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(4),
            Some("0.0.4".to_string()),
        );
        arch.add_library("aarch64-linux", b"heylookimaraspberrypi")?;
        arch.add_library("x86_64-linux", b"system76")?;
        arch.add_library("x86_64-macos", b"16inchmacbookpro")?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let filename = "computers";

        let tempdir = tempfile::tempdir()?;

        let parpath = tempdir.path().join(format!("{}.par", filename));
        let cheezypath = tempdir.path().join(format!("{}.par.gz", filename));

        arch.write(&parpath, &issuer, &subject, false).await?;
        arch.write(&cheezypath, &issuer, &subject, true).await?;

        let mut buf2 = Vec::new();
        let mut f2 = File::open(&parpath).await?;
        f2.read_to_end(&mut buf2).await?;

        let mut buf3 = Vec::new();
        let mut f3 = File::open(&cheezypath).await?;
        f3.read_to_end(&mut buf3).await?;

        // Make sure the file we wrote compressed can be read back in with no data loss
        let arch2 = ProviderArchive::try_load(&buf3).await?;
        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries[&"aarch64-linux".to_string()],
            arch2.libraries[&"aarch64-linux".to_string()]
        );
        assert_eq!(arch.claims().unwrap().subject, subject.public_key());

        // Try loading from file as well
        let arch2 = ProviderArchive::try_load_file(&cheezypath).await?;
        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries.get("aarch64-linux"),
            arch2.libraries.get("aarch64-linux")
        );
        assert_eq!(arch.claims().unwrap().subject, subject.public_key());

        Ok(())
    }

    #[tokio::test]
    async fn valid_write_compressed() -> Result<()> {
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(6),
            Some("0.0.6".to_string()),
        );
        arch.add_library("x86_64-linux", b"linux")?;
        arch.add_library("arm-macos", b"macos")?;
        arch.add_library("mips64-freebsd", b"freebsd")?;

        let filename = "multi-os";

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let tempdir = tempfile::tempdir()?;

        arch.write(
            tempdir.path().join(format!("{}.par", filename)),
            &issuer,
            &subject,
            true,
        )
        .await?;

        let arch2 =
            ProviderArchive::try_load_file(tempdir.path().join(format!("{}.par.gz", filename)))
                .await?;

        assert_eq!(
            arch.libraries[&"x86_64-linux".to_string()],
            arch2.libraries[&"x86_64-linux".to_string()]
        );
        assert_eq!(
            arch.libraries[&"arm-macos".to_string()],
            arch2.libraries[&"arm-macos".to_string()]
        );
        assert_eq!(
            arch.libraries[&"mips64-freebsd".to_string()],
            arch2.libraries[&"mips64-freebsd".to_string()]
        );
        assert_eq!(arch.claims(), arch2.claims());

        Ok(())
    }

    #[tokio::test]
    async fn valid_write_compressed_with_suffix() -> Result<()> {
        let mut arch = ProviderArchive::new(
            "wasmcloud:testing",
            "Testing",
            "wasmCloud",
            Some(7),
            Some("0.0.7".to_string()),
        );
        arch.add_library("x86_64-linux", b"linux")?;
        arch.add_library("arm-macos", b"macos")?;
        arch.add_library("mips64-freebsd", b"freebsd")?;

        let filename = "suffix-test";

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let tempdir = tempfile::tempdir()?;
        let cheezypath = tempdir.path().join(format!("{}.par.gz", filename));

        // the gz suffix is explicitly provided to write
        arch.write(&cheezypath, &issuer, &subject, true)
            .await
            .expect("Unable to write parcheezy");

        let arch2 = ProviderArchive::try_load_file(&cheezypath)
            .await
            .expect("Unable to load parcheezy from file");

        assert_eq!(
            arch.libraries[&"x86_64-linux".to_string()],
            arch2.libraries[&"x86_64-linux".to_string()]
        );
        assert_eq!(
            arch.libraries[&"arm-macos".to_string()],
            arch2.libraries[&"arm-macos".to_string()]
        );
        assert_eq!(
            arch.libraries[&"mips64-freebsd".to_string()],
            arch2.libraries[&"mips64-freebsd".to_string()]
        );
        assert_eq!(arch.claims(), arch2.claims());

        Ok(())
    }

    #[tokio::test]
    async fn preserved_claims() -> Result<()> {
        // Build an archive in memory the way a CLI wrapper might...
        let capid = "wasmcloud:testing";
        let name = "Testing";
        let vendor = "wasmCloud";
        let rev = 8;
        let ver = "0.0.8".to_string();
        let mut arch = ProviderArchive::new(capid, name, vendor, Some(rev), Some(ver.clone()));
        arch.add_library("aarch64-linux", b"blahblah")?;
        arch.add_library("x86_64-linux", b"bloobloo")?;
        arch.add_library("x86_64-macos", b"blarblar")?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        let tempdir = tempfile::tempdir()?;
        let originalpath = tempdir.path().join("original.par.gz");
        let addedpath = tempdir.path().join("linuxadded.par.gz");

        arch.write(&originalpath, &issuer, &subject, true).await?;

        // Make sure the file we wrote can be read back in with no claims loss
        let mut arch2 = ProviderArchive::try_load_file(&originalpath).await?;

        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries[&"aarch64-linux".to_string()],
            arch2.libraries[&"aarch64-linux".to_string()]
        );
        assert_eq!(arch2.claims().unwrap().subject, subject.public_key());
        assert_eq!(arch2.claims().unwrap().issuer, issuer.public_key());
        assert_eq!(arch2.claims().unwrap().name(), name);
        assert_eq!(arch2.claims().unwrap().metadata.unwrap().ver.unwrap(), ver);
        assert_eq!(arch2.claims().unwrap().metadata.unwrap().rev.unwrap(), rev);
        assert_eq!(arch2.claims().unwrap().metadata.unwrap().vendor, vendor);
        assert_eq!(arch2.claims().unwrap().metadata.unwrap().capid, capid);

        // Another common task - read an existing archive and add another library file to it
        arch2.add_library("mips-linux", b"bluhbluh")?;
        arch2.write(&addedpath, &issuer, &subject, true).await?;

        // Make sure the re-written/modified archive looks the way we expect
        let arch3 = ProviderArchive::try_load_file(&addedpath).await?;
        assert_eq!(arch3.capid, arch2.capid);
        assert_eq!(
            arch3.libraries[&"aarch64-linux".to_string()],
            arch2.libraries[&"aarch64-linux".to_string()]
        );
        assert_eq!(arch3.claims().unwrap().subject, subject.public_key());
        assert_eq!(arch3.claims().unwrap().issuer, issuer.public_key());
        assert_eq!(arch3.claims().unwrap().name(), name);
        assert_eq!(arch3.claims().unwrap().metadata.unwrap().ver.unwrap(), ver);
        assert_eq!(arch3.claims().unwrap().metadata.unwrap().rev.unwrap(), rev);
        assert_eq!(arch3.claims().unwrap().metadata.unwrap().vendor, vendor);
        assert_eq!(arch3.claims().unwrap().metadata.unwrap().capid, capid);
        assert_eq!(arch3.targets().len(), 4);

        Ok(())
    }
}
