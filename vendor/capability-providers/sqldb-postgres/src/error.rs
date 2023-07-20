use wasmcloud_interface_sqldb::SqlDbError;

/// Errors reported by this provider
#[non_exhaustive]
#[allow(dead_code)]
pub(crate) enum DbError {
    /// Database error
    Db(String),
    /// Error communicating with the database
    Io(String),
    /// Configuration error
    Config(String),
    /// Error encoding results
    Encoding(String),
    /// No rows returned when a result was expected
    NotFound(String),
    /// Error encountered in capability provider
    Provider(String),
    /// Error that could not be categorized as one of the above
    Other(String),
}

/*
/// convert cbor encoding errors to DbError
impl<W: std::io::Write> From<minicbor::encode::Error<W>> for DbError {
    fn from(e: minicbor::encode::Error<W>) -> DbError
    where
        W: minicbor::encode::Write,
    {
        DbError::Encoding(e.to_string())
    }
}
 */

/// convert std::io errors to DbError
impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> DbError {
        DbError::Encoding(e.to_string())
    }
}

/// convert tokio-postgres errors to DbError
impl From<tokio_postgres::Error> for DbError {
    fn from(e: tokio_postgres::Error) -> DbError {
        DbError::Db(e.to_string())
    }
}

/// convert tokio-postgres db error messages to DbError
impl From<tokio_postgres::error::DbError> for DbError {
    fn from(e: tokio_postgres::error::DbError) -> DbError {
        // format: SEVERITY: message \n detail \n hint
        DbError::Db(format!("{}", e))
    }
}

/// convert connection-pool errors to DbError
impl<E: std::error::Error + 'static> From<bb8_postgres::bb8::RunError<E>> for DbError {
    fn from(e: bb8_postgres::bb8::RunError<E>) -> DbError {
        DbError::Io(e.to_string())
    }
}

/// convert DbError to the sqldb interface-defined error for client return
impl From<DbError> for SqlDbError {
    fn from(e: DbError) -> SqlDbError {
        match e {
            DbError::Db(s) => SqlDbError::new("db", s),
            DbError::Io(s) => SqlDbError::new("io", s),
            DbError::Config(s) => SqlDbError::new("config", s),
            DbError::Encoding(s) => SqlDbError::new("encoding", s),
            DbError::NotFound(s) => SqlDbError::new("notFound", s),
            DbError::Provider(s) => SqlDbError::new("provider", s),
            DbError::Other(s) => SqlDbError::new("other", s),
        }
    }
}
