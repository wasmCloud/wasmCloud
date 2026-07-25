//! File-backing for compiled components so instantiation can use
//! copy-on-write memory images.
//!
//! Wasmtime can only build a CoW memory image for a component's initialized
//! data when the compiled artifact is either mmapped from a file or, on Linux
//! only, copied into an anonymous `memfd`. A component compiled in memory via
//! `Component::new` gets neither on macOS/Windows, so every instantiation
//! eagerly copies all initialized data (~110-140 us per MiB). For components
//! with large data segments (JS/Python/.NET guest runtimes) that copy
//! dominates cross-component call latency.
//!
//! [`file_backed_component`] round-trips a freshly compiled component through
//! a short-lived file: serialize, write to a process-private directory, load
//! back with `Component::deserialize_file`, then delete the file immediately.
//! Wasmtime keeps the file descriptor open for the component's lifetime and
//! maps each new instance's memory image from it, so the deleted file's
//! blocks live on (invisible to `ls`, reclaimed by the kernel when the
//! component drops — or at process exit, even on a crash). No persistent
//! cache directory ever exists.
//!
//! Set `WASH_NO_FILE_BACKED_COMPONENTS=1` to keep components in memory.

// `Component::deserialize_file` is `unsafe` because it trusts its input to be
// a valid artifact for this engine. Safety here rests on only ever loading a
// file this process wrote moments earlier: created with `create_new` (so no
// pre-existing file or symlink is followed) inside a directory created by
// this process with mode 0o700, written from bytes produced by
// `Component::serialize` on the same engine, and deleted right after loading.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Context as _;
use tracing::{debug, warn};
use wasmtime::component::Component;

/// Environment variable that disables file-backing entirely.
const DISABLE_ENV: &str = "WASH_NO_FILE_BACKED_COMPONENTS";

/// Process-private directory holding backing files for the instant between
/// write and delete. Created once, lazily; `None` if creation failed (the
/// engine then just keeps components in memory).
fn backing_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "wash-cwasm-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                warn!(dir = %dir.display(), error = %e, "failed to create component file-backing directory, components will stay in-memory");
                None
            }
        }
    })
    .as_ref()
}

/// Re-load a freshly compiled component from a short-lived file so wasmtime
/// can serve instantiations from a CoW memory image.
///
/// Falls back to the given in-memory component (with a warning) on any
/// failure, and is a no-op when [`DISABLE_ENV`] is set.
pub(crate) fn file_backed_component(engine: &wasmtime::Engine, component: Component) -> Component {
    if std::env::var_os(DISABLE_ENV).is_some_and(|v| !v.is_empty()) {
        return component;
    }
    match try_file_backed(engine, &component) {
        Ok(backed) => backed,
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                "failed to file-back compiled component, using in-memory artifact (instantiation of large components may be slow)"
            );
            component
        }
    }
}

fn try_file_backed(engine: &wasmtime::Engine, component: &Component) -> anyhow::Result<Component> {
    let dir = backing_dir().context("no file-backing directory available")?;
    let artifact = component
        .serialize()
        .map_err(anyhow::Error::from)
        .context("failed to serialize compiled component")?;
    let path = dir.join(format!("{}.cwasm", uuid::Uuid::new_v4()));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }

    let loaded = options
        .open(&path)
        .with_context(|| format!("failed to create component backing file {}", path.display()))
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(&artifact)
                .context("failed to write component backing file")?;
            drop(file);
            // SAFETY: `path` was created just above with `create_new` inside a
            // 0o700 process-private directory and contains exactly the bytes
            // `Component::serialize` produced for this engine; nothing else
            // can have opened or replaced it.
            unsafe { Component::deserialize_file(engine, &path) }
                .map_err(anyhow::Error::from)
                .context("failed to deserialize file-backed component")
        });

    // Delete the file regardless of outcome. On success wasmtime holds an
    // open descriptor, so the mapping (and the file's blocks) outlive the
    // directory entry.
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!(path = %path.display(), error = %e, "failed to remove component backing file");
    }

    if loaded.is_ok() {
        debug!(bytes = artifact.len(), "file-backed compiled component");
    }
    loaded
}
