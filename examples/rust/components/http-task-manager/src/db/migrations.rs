use anyhow::{bail, Context as _, Result};
use include_dir::{include_dir, Dir, File};

use crate::bindings::wasi::logging::logging::{log, Level};
use crate::bindings::wasmcloud::postgres::query::query_batch;
use crate::LOG_CONTEXT;

static MIGRATION_DIR: Dir = include_dir!("sql/migrations");

/// A simple mechanism for retrieving all migrations and running them in lexicographic order.
///
/// The simple intent behind this function is to get all migrations, and likely
/// run them all as statements against an online database.
///
/// For this simplistic method to work well, all migrations must be:
///
/// - written idempotently (all migrations should be able to be run repeatedly)
/// - majority written as transactions (Postgres, unlike other databases supports transactional DDL)
///   - the exception here is statements like `CREATE EXTENSION`
/// - Written in a resource efficient way (i.e. use of concurrent indexes)
/// - Triggerable separately from app startup
/// - Always backwards compatible with existing infrastructure (i.e. mostly accretive changes only)
///
pub(crate) fn perform_migrations() -> Result<()> {
    // Retrieve lexicographically sorted list of top level SQL files
    let mut files: Vec<&File<'_>> = MIGRATION_DIR
        .find("*.sql")
        .context("failed to get perform find glob")?
        .flat_map(|de| de.as_file())
        .collect::<Vec<_>>();
    files.sort_by_key(|a| a.path().file_name());

    // Execute SQL in all files
    for f in files.iter() {
        let filename = f
            .path()
            .file_name()
            .and_then(|v| v.to_str())
            .context("failed to get migration file name")?;
        log(
            Level::Debug,
            LOG_CONTEXT,
            &format!("executing migration in file [{filename}]"),
        );
        let migration_sql = f
            .contents_utf8()
            .with_context(|| format!("failed to read sql from file [{filename}]"))?;
        if let Err(e) = query_batch(migration_sql) {
            log(
                Level::Error,
                LOG_CONTEXT,
                &format!("failed to execute migration [{filename}]: {e}"),
            );
            bail!("failed to execute migration [{filename}]: {e}");
        }

        log(
            Level::Info,
            LOG_CONTEXT,
            &format!("successfully completed migration [{filename}]"),
        );
    }

    Ok(())
}
