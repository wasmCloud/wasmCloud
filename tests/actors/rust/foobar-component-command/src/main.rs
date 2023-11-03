use std::env::args;
use std::io::{stdin, stdout};

use anyhow::Context;

// TODO: Migrate this to Go

fn main() -> anyhow::Result<()> {
    let mut args = args();
    assert_eq!(args.next().as_deref(), Some("main.wasm"));
    // TODO: This should include the package name, i.e. `test-actors:foobar/foobar.foobar`
    assert_eq!(args.next().as_deref(), Some("foobar.foobar"));
    assert!(args.next().is_none());
    let arg: String = rmp_serde::from_read(stdin().lock()).context("failed to read input")?;
    assert_eq!(arg, "foo");
    rmp_serde::encode::write(&mut stdout().lock(), "foobar").context("failed to write output")?;
    Ok(())
}
