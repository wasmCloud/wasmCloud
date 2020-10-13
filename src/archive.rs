use crate::Result;
use data_encoding::HEXUPPER;
use ring::digest::{Context, Digest, SHA256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use wascap::jwt::{CapabilityProvider, Claims};
use wascap::prelude::KeyPair;

const CLAIMS_JWT_FILE: &str = "claims.jwt";

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
        }
    }

    /// Adds a native library file (.so, .dylib, .dll) to the archive for a given target string
    pub fn add_library(&mut self, target: &str, input: &[u8]) -> Result<()> {
        self.libraries.insert(target.to_string(), input.to_vec());

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

    /// Attempts to read a Provider Archive (PAR) file's bytes to analyze and verify its contents. The embedded claims
    /// in this archive will be validated, and the file hashes contained in those claims will be compared and
    /// verified against hashes computed at load time. This prevents the contents of the archive from being modified
    /// without the embedded claims being re-signed
    pub fn try_load(input: &[u8]) -> Result<ProviderArchive> {
        let reader = std::io::Cursor::new(input);

        let mut libraries = HashMap::new();
        let mut zip = zip::ZipArchive::new(reader)?;
        let mut c: Option<Claims<CapabilityProvider>> = None;
        if zip.len() < 2 {
            // we need at least claims.jwt and one plugin binary
            return Err(
                "Not enough files found in provider archive. Is this a complete archive?".into(),
            );
        }
        for i in 0..zip.len() {
            let file = zip.by_index(i).unwrap();
            let target = PathBuf::from(file.name())
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let mut filebuf = Vec::new();
            for byte in file.bytes() {
                filebuf.push(byte?);
            }
            if target == "claims" {
                c = Some(Claims::<CapabilityProvider>::decode(&std::str::from_utf8(
                    &filebuf,
                )?)?);
            } else {
                libraries.insert(target.to_string(), filebuf.to_vec());
            }
        }

        if let Some(ref cl) = c {
            let capid = cl.metadata.as_ref().unwrap().capid.to_string();
            let name = cl.name();
            let vendor = cl.metadata.as_ref().unwrap().vendor.to_string();

            validate_hashes(&libraries, c.as_ref().unwrap())?;

            Ok(ProviderArchive {
                libraries,
                capid,
                name,
                vendor,
                rev: None,
                ver: None,
                claims: c,
            })
        } else {
            Err("No claims found embedded in provider archive.".into())
        }
    }

    /// Generates a Provider Archive (PAR) file with all of the library files and a signed set of claims in an embedded JWT
    pub fn write(
        &mut self,
        destination: &mut File,
        issuer: &KeyPair,
        subject: &KeyPair,
    ) -> Result<()> {
        let hashes = generate_hashes(&self.libraries);
        let mut zip = zip::ZipWriter::new(destination);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        let claims = Claims::<CapabilityProvider>::new(
            self.name.to_string(),
            issuer.public_key(),
            subject.public_key(),
            self.capid.to_string(),
            self.vendor.to_string(),
            self.rev.clone(),
            self.ver.clone(),
            hashes,
        );
        self.claims = Some(claims.clone());
        zip.start_file(CLAIMS_JWT_FILE, options)?;
        zip.write(claims.encode(&issuer)?.as_bytes())?;

        for (tgt, lib) in self.libraries.iter() {
            zip.start_file(format!("{}.bin", tgt), options)?;
            zip.write(lib)?;
        }

        zip.finish()?;

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
        let check_hash = hash_bytes(&library);
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
    use crate::ProviderArchive;
    use crate::Result;
    use std::fs::File;
    use std::io::Read;
    use wascap::prelude::KeyPair;

    #[test]
    fn round_trip() -> Result<()> {
        // Build an archive in memory the way a CLI wrapper might...
        let mut arch = ProviderArchive::new(
            "wascc:testing",
            "Testing",
            "waSCC",
            Some(1),
            Some("0.0.1".to_string()),
        );
        arch.add_library("aarch64-linux", b"blahblah")?;
        arch.add_library("x86_64-linux", b"bloobloo")?;
        arch.add_library("x86_64-macos", b"blarblar")?;

        let issuer = KeyPair::new_account();
        let subject = KeyPair::new_service();

        // Generate the .par file with embedded claims.jwt file (needs a service and an account key)
        let mut f = File::create("./test.par")?;
        arch.write(&mut f, &issuer, &subject)?;

        let mut buf2 = Vec::new();
        let mut f2 = File::open("./test.par")?;
        f2.read_to_end(&mut buf2)?;

        // Make sure the file we wrote can be read back in with no data loss
        let mut arch2 = ProviderArchive::try_load(&buf2)?;
        assert_eq!(arch.capid, arch2.capid);
        assert_eq!(
            arch.libraries[&"aarch64-linux".to_string()].len(),
            arch2.libraries[&"aarch64-linux".to_string()].len()
        );
        assert_eq!(arch.claims().unwrap().subject, subject.public_key());

        // Another common task - read an existing archive and add another library file to it
        arch2.add_library("mips-linux", b"bluhbluh")?;
        let mut f3 = File::create("./test2.par")?;
        arch2.write(&mut f3, &issuer, &subject)?;

        let mut buf3 = Vec::new();
        let mut f4 = File::open("./test2.par")?;
        f4.read_to_end(&mut buf3)?;

        // Make sure the re-written/modified archive looks the way we expect
        let arch3 = ProviderArchive::try_load(&buf3)?;
        assert_eq!(arch3.capid, arch2.capid);
        assert_eq!(
            arch3.libraries[&"aarch64-linux".to_string()].len(),
            arch2.libraries[&"aarch64-linux".to_string()].len()
        );
        assert_eq!(arch3.claims().unwrap().subject, subject.public_key());
        assert_eq!(arch3.targets().len(), 4);

        let _ = std::fs::remove_file("./test.par");
        let _ = std::fs::remove_file("./test2.par");

        Ok(())
    }
}
