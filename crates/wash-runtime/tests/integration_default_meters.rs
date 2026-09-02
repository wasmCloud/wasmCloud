//! A host built without `with_meters` publishes the process-wide invocation
//! meter, the way a CLI-built one does.
//!
//! Its own binary, and one test. `invocation_meter()` reads a process-wide
//! `OnceLock` that any `Meters::new` with a recording kind fills, so a sibling
//! test in the same binary that built its own meters would set it first and
//! this assertion would hold whatever `HostBuilder` did with its default. The
//! enabled-ness of a host's *own* meters is instance-local and pinned by the
//! unit tests in `host::tests`; only this one needs the isolation.

use anyhow::Result;
use wash_runtime::host::Host;
use wash_runtime::observability::invocation_meter;

#[test]
fn the_builder_default_publishes_the_invocation_meter() -> Result<()> {
    assert!(
        invocation_meter().is_none(),
        "nothing in this binary has built a recording meter yet, so the global must be empty — \
         without that, what follows proves nothing"
    );

    let _host = Host::builder().build()?;

    assert!(
        invocation_meter().is_some(),
        "a host built without `with_meters` must resolve its meters through `Meters::new`, which \
         is what publishes the meter every call path that cannot reach a host's own meters reads"
    );

    Ok(())
}
