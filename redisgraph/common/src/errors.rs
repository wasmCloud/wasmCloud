#[doc(hidden)]
#[macro_export]
macro_rules! client_type_error {
    ($($arg:tt)*) => {
        Err($crate::GraphError::ClientTypeError(format!($($arg)*)))
    };
}

/// Common result type for this crate.
pub type GraphResult<T> = Result<T, GraphError>;

/// Common error type for this crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GraphError {
    /// Returned if the data you requested is of a different type
    /// than the data returned by the database.
    ClientTypeError(String),

    /// Returned if a label name was not found in the graph's internal registry.
    ///
    /// This error is taken care of by the implementation and should never reach your code.
    LabelNotFound,
    /// Returned if a relationship type name was not found in the graph's internal registry.
    ///
    /// This error is taken care of by the implementation and should never reach your code.
    RelationshipTypeNotFound,
    /// Returned if a property key name was not found in the graph's internal registry.
    ///
    /// This error is taken care of by the implementation and should never reach your code.
    PropertyKeyNotFound,

    /// Returned if you requested a [`String`](https://doc.rust-lang.org/std/string/struct.String.html) and the database responded with bytes that are invalid UTF-8.
    ///
    /// If you don't care about whether the data is valid UTF-8, consider requesting a [`RedisString`](../result_set/struct.RedisString.html) instead.
    InvalidUtf8,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphError::ClientTypeError(e) => write!(f, "Graph client error: {}", e),
            _ => write!(f, "Graph error"),
        }
    }
}
