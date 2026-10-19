//! Getting the pieces a run needs: the host binary, nats-server and the
//! component, cached so a second run downloads and builds nothing.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// The workspace root this bench was built in (the directory holding
/// `Cargo.lock`), or the current directory when it has been moved.
pub fn workspace_dir() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("Cargo.lock").exists() {
            return dir;
        }
        if !dir.pop() {
            return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        }
    }
}

/// The target triple wash releases are named by, as install.sh picks it (and
/// the Windows build it does not offer).
pub fn wash_target() -> Result<&'static str> {
    wash_target_for(
        std::env::consts::ARCH,
        std::env::consts::OS,
        cfg!(target_env = "musl"),
    )
}

fn wash_target_for(arch: &str, os: &str, musl: bool) -> Result<&'static str> {
    Ok(match (arch, os) {
        ("x86_64", "linux") if musl => "x86_64-unknown-linux-musl",
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") if musl => "aarch64-unknown-linux-musl",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("s390x", "linux") if !musl => "s390x-unknown-linux-gnu",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        (arch, os) => bail!("no released wash for {arch}-{os}; pass --host-bin"),
    })
}

pub fn wash_url(version: &str, target: &str) -> String {
    format!("https://github.com/wasmCloud/wasmCloud/releases/download/v{version}/wash-{target}")
}

/// The `{os}-{arch}` nats-server names its release archives by.
pub fn nats_platform() -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => bail!("no nats-server download for {other}; pass --nats-url"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => bail!("no nats-server download for {other}; pass --nats-url"),
    };
    Ok(format!("{os}-{arch}"))
}

pub fn nats_url(version: &str, platform: &str) -> String {
    format!(
        "https://github.com/nats-io/nats-server/releases/download/v{version}/nats-server-v{version}-{platform}.tar.gz"
    )
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("bench-scale/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building the download client")
}

/// GET `url` into memory.
async fn download(url: &str) -> Result<Vec<u8>> {
    let mut response = http_client()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("downloading {url}"))?
    {
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Write `bytes` to `dest` as an executable, through a temporary file so an
/// interrupted run never leaves a truncated binary in the cache.
fn install_executable(dest: &Path, bytes: &[u8]) -> Result<()> {
    let dir = dest.parent().context("install path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let partial = dest.with_extension(format!("partial-{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&partial, dest).with_context(|| format!("installing {}", dest.display()))?;
    Ok(())
}

/// The released wash binary for `version`, downloaded once.
pub async fn ensure_wash(cache: &Path, version: &str, say: &dyn Fn(&str)) -> Result<PathBuf> {
    let target = wash_target()?;
    let dest = cache
        .join("wash")
        .join(version)
        .join(format!("wash-{target}{}", std::env::consts::EXE_SUFFIX));
    if dest.exists() {
        return Ok(dest);
    }
    let url = wash_url(version, target);
    say(&format!("downloading {url}"));
    let bytes = download(&url).await?;
    install_executable(&dest, &bytes)?;
    Ok(dest)
}

/// nats-server `version`, downloaded and unpacked once.
pub async fn ensure_nats(cache: &Path, version: &str, say: &dyn Fn(&str)) -> Result<PathBuf> {
    let dest = cache.join("nats-server").join(version).join("nats-server");
    if dest.exists() {
        return Ok(dest);
    }
    let url = nats_url(version, &nats_platform()?);
    say(&format!("downloading {url}"));
    let archive = download(&url).await?;
    let binary = extract_nats_server(&archive).with_context(|| format!("unpacking {url}"))?;
    install_executable(&dest, &binary)?;
    Ok(dest)
}

/// The `nats-server` binary out of a release `.tar.gz`. Reads just that entry
/// instead of unpacking the archive, so no path in it is ever written.
pub fn extract_nats_server(archive: &[u8]) -> Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let is_binary = entry.header().entry_type().is_file()
            && entry
                .path()?
                .file_name()
                .is_some_and(|n| n == "nats-server");
        if is_binary {
            let mut out = Vec::new();
            entry.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    bail!("no nats-server binary in the archive")
}

/// The local `path = "…"` entries of a fixture's `wkg.toml`: the shared WIT
/// packages `wash build` would fetch into `wit/deps`.
pub fn wkg_paths(wkg_toml: &str) -> Vec<String> {
    wkg_toml
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let (_, rest) = line.split_once("path")?;
            let rest = rest.trim_start().strip_prefix('=')?.trim_start();
            let rest = rest.strip_prefix('"')?;
            let (path, _) = rest.split_once('"')?;
            Some(path.to_string())
        })
        .collect()
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

pub const FIXTURE: &str = "http-handler-p3";

/// Build the poolable P3 fixture without wash: put the WIT packages its
/// `wkg.toml` names into `wit/deps` (what `wash build`'s fetch does), then
/// compile a wasip1 core module and wrap it with the same reactor adapter as wash.
pub async fn build_fixture(workspace: &Path, say: &dyn Fn(&str)) -> Result<PathBuf> {
    let fixtures = workspace.join("crates/wash-runtime/tests/fixtures");
    let fixture = fixtures.join(FIXTURE);
    let wkg = std::fs::read_to_string(fixture.join("wkg.toml"))
        .with_context(|| format!("reading {}/wkg.toml", fixture.display()))?;
    let deps = fixture.join("wit/deps");
    for path in wkg_paths(&wkg) {
        let source = fixture.join(&path);
        let name = source
            .file_name()
            .with_context(|| format!("wkg.toml path {path} has no name"))?;
        let target = deps.join(name);
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        copy_dir(&source, &target)
            .with_context(|| format!("copying {} into {}", source.display(), deps.display()))?;
    }

    say(&format!("building the {FIXTURE} fixture"));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-wasip1",
            "-p",
            FIXTURE,
        ])
        .current_dir(&fixtures)
        // The fixtures are their own workspace; an inherited target dir would
        // put the component somewhere this does not look.
        .env_remove("CARGO_TARGET_DIR");
    let output = crate::process::output(command, std::time::Duration::from_secs(600))
        .await
        .context("running cargo build for the fixture")?;
    if !output.status.success() {
        bail!(
            "building {FIXTURE} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let wasm = fixtures.join("target/wasm32-wasip1/release/http_handler_p3.wasm");
    if !wasm.exists() {
        bail!("cargo built {FIXTURE} but {} is missing", wasm.display());
    }
    let component = wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&std::fs::read(&wasm)?)?
        .adapter(
            "wasi_snapshot_preview1",
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
        )?
        .encode()
        .context("wrapping the P3 fixture as a component")?;
    validate_poolable_http(&component)?;
    let path = wasm.with_extension("component.wasm");
    std::fs::write(&path, component)?;
    Ok(path)
}

/// The experiment needs a reusable P3 HTTP handler, not a single-use P2 store.
pub fn validate_poolable_http(bytes: &[u8]) -> Result<()> {
    let wit_component::DecodedWasm::Component(resolve, world) = wit_component::decode(bytes)?
    else {
        bail!("the benchmark requires a P3 HTTP component");
    };
    let world = resolve
        .worlds
        .get(world)
        .context("component world is missing")?;
    if world
        .exports
        .keys()
        .any(|key| resolve.name_world_key(key) == "wasi:http/handler@0.3.0")
    {
        return Ok(());
    }
    bail!(
        "the benchmark requires a poolable wasi:http/handler@0.3.0 export; P2 HTTP instances cannot stay warm"
    )
}

/// Pool slots sized the way a large host sizes them: half the usable
/// address space at wasmtime's 4 GiB-plus-32 MiB-guard slot, and no more than
/// the kernel's mapping limit allows at about five mappings a slot.
pub fn derive_core_instances(address_space: u64, max_map_count: Option<u64>) -> u32 {
    const SLOT: u64 = (4 << 30) + (32 << 20);
    let by_address_space = address_space / 2 / SLOT;
    let by_map_count = max_map_count.map_or(u64::MAX, |limit| limit / 5);
    u32::try_from(by_address_space.min(by_map_count).max(1)).unwrap_or(u32::MAX)
}

/// The soft `Max address space` limit, or 128 TiB (the 64-bit user address
/// space) when unlimited or unknown.
pub fn usable_address_space() -> u64 {
    const USER_ADDRESS_SPACE: u64 = 128 << 40;
    std::fs::read_to_string("/proc/self/limits")
        .ok()
        .and_then(|limits| soft_address_space_limit(&limits))
        .map_or(USER_ADDRESS_SPACE, |limit| limit.min(USER_ADDRESS_SPACE))
}

fn soft_address_space_limit(limits: &str) -> Option<u64> {
    limits
        .lines()
        .find_map(|line| line.strip_prefix("Max address space"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

pub fn max_map_count() -> Option<u64> {
    std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_component_without_a_p3_handler_is_rejected() {
        assert!(validate_poolable_http(b"\0asm\x0d\0\x01\0").is_err());
        assert!(validate_poolable_http(b"not a component").is_err());
    }

    #[tokio::test]
    #[ignore = "builds the fixture; requires the wasm32-wasip1 Rust target"]
    async fn the_default_fixture_has_a_poolable_http_export() -> Result<()> {
        let path = build_fixture(&workspace_dir(), &|line| eprintln!("{line}")).await?;
        validate_poolable_http(&std::fs::read(path)?)
    }

    #[test]
    fn release_urls() {
        assert_eq!(
            wash_url("2.10.3", "x86_64-unknown-linux-gnu"),
            "https://github.com/wasmCloud/wasmCloud/releases/download/v2.10.3/wash-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            nats_url("2.12.8", "linux-amd64"),
            "https://github.com/nats-io/nats-server/releases/download/v2.12.8/nats-server-v2.12.8-linux-amd64.tar.gz"
        );
    }

    #[test]
    fn release_targets_do_not_depend_on_the_test_platform() -> Result<()> {
        for (arch, os, musl, expected) in [
            ("x86_64", "linux", false, "x86_64-unknown-linux-gnu"),
            ("x86_64", "linux", true, "x86_64-unknown-linux-musl"),
            ("aarch64", "linux", false, "aarch64-unknown-linux-gnu"),
            ("aarch64", "linux", true, "aarch64-unknown-linux-musl"),
            ("s390x", "linux", false, "s390x-unknown-linux-gnu"),
            ("x86_64", "macos", false, "x86_64-apple-darwin"),
            ("aarch64", "macos", false, "aarch64-apple-darwin"),
            ("x86_64", "windows", false, "x86_64-pc-windows-msvc"),
        ] {
            assert_eq!(wash_target_for(arch, os, musl)?, expected);
        }
        for (arch, os, musl) in [
            ("aarch64", "windows", false),
            ("s390x", "linux", true),
            ("riscv64", "linux", false),
        ] {
            let error = wash_target_for(arch, os, musl).expect_err("no download is supported");
            assert!(error.to_string().contains("--host-bin"));
        }
        Ok(())
    }

    #[test]
    fn reads_wkg_overrides() {
        let wkg = r#"[overrides]
"wasi:cli" = { path = "../p2-wit-deps/wasi-cli-0.2.2" }
"wasi:http" = { path = "../p2-wit-deps/wasi-http-0.2.2" }
# path = "commented"
"#;
        let paths = wkg_paths(wkg);
        assert!(paths.contains(&"../p2-wit-deps/wasi-cli-0.2.2".to_string()));
        assert!(paths.contains(&"../p2-wit-deps/wasi-http-0.2.2".to_string()));
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn core_instances_like_a_large_host() {
        // 128 TiB / 2 at 4 GiB + 32 MiB a slot.
        assert_eq!(derive_core_instances(128 << 40, None), 16_256);
        // The default vm.max_map_count binds first.
        assert_eq!(derive_core_instances(128 << 40, Some(65_530)), 13_106);
        assert_eq!(derive_core_instances(0, None), 1);
    }

    #[test]
    fn address_space_limit() {
        let limits = "Limit                     Soft Limit           Hard Limit           Units\n\
                      Max address space         unlimited            unlimited            bytes\n";
        assert_eq!(soft_address_space_limit(limits), None);
        let limits = "Max address space         8589934592           unlimited            bytes\n";
        assert_eq!(soft_address_space_limit(limits), Some(8 << 30));
    }

    #[test]
    fn extracts_only_the_binary() {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in [
            ("nats-server-v1-linux-amd64/README.md", &b"readme"[..]),
            (
                "nats-server-v1-linux-amd64/nats-server",
                &b"\x7fELF binary"[..],
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, body).unwrap();
        }
        let tarball = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tarball).unwrap();
        let archive = gz.finish().unwrap();
        assert_eq!(extract_nats_server(&archive).unwrap(), b"\x7fELF binary");
    }
}
