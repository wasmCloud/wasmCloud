use anyhow::Context;

fn main() -> anyhow::Result<()> {
    wit_deps::lock_sync!().context("failed to lock WIT dependencies")?;

    println!("cargo:rerun-if-changed=../../../wit");

    println!("cargo:rerun-if-changed=wit/deps");
    println!("cargo:rerun-if-changed=wit/deps.lock");
    println!("cargo:rerun-if-changed=wit/deps.toml");

    Ok(())
}
