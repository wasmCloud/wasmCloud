use std::env::args;
use std::io::{stdin, stdout};

use anyhow::Context;
use serde::Deserialize;

// TODO: Migrate this to Go

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FoobarInput {
    arg: String,
}

fn main() -> anyhow::Result<()> {
    let mut args = args();
    assert_eq!(args.next().as_deref(), Some("main.wasm"));
    // TODO: This should include the package name, i.e. `test-actors:foobar/actor.foobar`
    assert_eq!(args.next().as_deref(), Some("actor.foobar"));
    assert!(args.next().is_none());
    let FoobarInput { arg } =
        serde_json::from_reader(stdin().lock()).context("failed to read input")?;
    assert_eq!(arg, "foo");
    serde_json::to_writer(stdout().lock(), "foobar").context("failed to write output")?;
    Ok(())
}
