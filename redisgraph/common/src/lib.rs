//! # Common types (GraphDB)
//!
//! A set of common types that largely support the `ResultSet` type, a wrapper
//! around results that come back from a graph database that supports dynamic,
//! strongly-typed tuple extraction.
//!
//! These types are mostly copied wholesale from the RedisGraph client library
//! that can be found at https://github.com/malte-v/redisgraph-rs

use std::collections::HashMap;

#[macro_use]
extern crate serde_derive;

mod conversions;
mod errors;
pub mod protocol;

pub use crate::errors::GraphResult;
pub use errors::GraphError;

pub const CAPID_GRAPHDB: &str = "wascc:graphdb";

/// Represents the return data from a graph. You shouldn't have to use this
/// type directly, but rather extract rows and columns via vectors of tuples
/// and pattern matching/destructing
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultSet {
    /// The columns of this result set.
    ///     
    /// Empty if the response did not contain any return values.
    pub columns: Vec<Column>,
    /// Contains statistics messages from the response.
    pub statistics: Statistics,
}

/// Human-readable statistics that are optionally returned with each query
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statistics(pub Vec<String>);

impl ResultSet {
    /// Returns the number of rows in the result set.
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// Returns the number of columns in the result set.
    pub fn num_rows(&self) -> usize {
        match self.columns.get(0) {
            Some(first_column) => first_column.len(),
            None => 0,
        }
    }

    /// Returns the scalar at the given position.
    ///
    /// Returns an error if the value at the given position is not a scalar
    /// or if the position is out of bounds.
    pub fn get_scalar(&self, row_idx: usize, column_idx: usize) -> GraphResult<&Scalar> {
        match self.columns.get(column_idx) {
            Some(column) => match column {
                Column::Scalars(cells) => match cells.get(row_idx) {
                    Some(cell) => Ok(cell),
                    None => client_type_error!(
                        "failed to get scalar: row index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
                    ),
                },
                any => client_type_error!(
                    "failed to get scalar: expected column of scalars, found {:?}",
                    any
                ),
            }
            None => client_type_error!(
                "failed to get scalar: column index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
            ),
        }
    }

    /// Returns the node at the given position.
    ///
    /// Returns an error if the value at the given position is not a node
    /// or if the position is out of bounds.
    pub fn get_node(&self, row_idx: usize, column_idx: usize) -> GraphResult<&Node> {
        match self.columns.get(column_idx) {
            Some(column) => match column {
                Column::Nodes(cells) => match cells.get(row_idx) {
                    Some(cell) => Ok(cell),
                    None => client_type_error!(
                        "failed to get node: row index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
                    ),
                },
                any => client_type_error!(
                    "failed to get node: expected column of nodes, found {:?}",
                    any
                ),
            }
            None => client_type_error!(
                "failed to get node: column index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
            ),
        }
    }

    /// Returns the relation at the given position.
    ///
    /// Returns an error if the value at the given position is not a relation
    /// or if the position is out of bounds.
    pub fn get_relation(&self, row_idx: usize, column_idx: usize) -> GraphResult<&Relation> {
        match self.columns.get(column_idx) {
            Some(column) => match column {
                Column::Relations(cells) => match cells.get(row_idx) {
                    Some(cell) => Ok(cell),
                    None => client_type_error!(
                        "failed to get relation: row index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
                    ),
                },
                any => client_type_error!(
                    "failed to get relation: expected column of relations, found {:?}",
                    any
                ),
            }
            None => client_type_error!(
                "failed to get relation: column index out of bounds: the len is {:?} but the index is {:?}", self.columns.len(), column_idx,
            ),
        }
    }
}

impl FromTable for ResultSet {
    fn from_table(result_set: &ResultSet) -> GraphResult<Self> {
        Ok(result_set.clone())
    }
}

impl<T: FromRow> FromTable for Vec<T> {
    fn from_table(result_set: &ResultSet) -> GraphResult<Self> {
        let num_rows = result_set.num_rows();
        let mut ret = Self::with_capacity(num_rows);

        for i in 0..num_rows {
            ret.push(T::from_row(result_set, i)?);
        }

        Ok(ret)
    }
}

pub trait FromTable: Sized {
    fn from_table(result_set: &ResultSet) -> GraphResult<Self>;
}

/// Implemented by types that can be constructed from a row in a [`ResultSet`](../result_set/struct.ResultSet.html).
pub trait FromRow: Sized {
    fn from_row(result_set: &ResultSet, row_idx: usize) -> GraphResult<Self>;
}

/// Implemented by types that can be constructed from a cell in a [`ResultSet`](../result_set/struct.ResultSet.html).
pub trait FromCell: Sized {
    fn from_cell(result_set: &ResultSet, row_idx: usize, column_idx: usize) -> GraphResult<Self>;
}

/// A single column of the result set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Column {
    Scalars(Vec<Scalar>),
    Nodes(Vec<Node>),
    Relations(Vec<Relation>),
}

impl Column {
    /// Returns the length of this column.
    pub fn len(&self) -> usize {
        match self {
            Self::Scalars(cells) => cells.len(),
            Self::Nodes(cells) => cells.len(),
            Self::Relations(cells) => cells.len(),
        }
    }

    /// Returns `true` if this column is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Serialize, Debug, Deserialize)]
enum ColumnType {
    Unknown = 0,
    Scalar = 1,
    Node = 2,
    Relation = 3,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// The type name of this relation.
    pub type_name: String,
    /// The properties of this relation.
    pub properties: HashMap<String, Scalar>,
}

/// A scalar value returned by the Graph provider
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scalar {
    Nil,
    Boolean(bool),
    Integer(i64),
    Double(f64),
    String(GraphString), // A string returned by the graph DB
}

/// The valid types of scalars
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ScalarType {
    Unknown = 0,
    Nil = 1,
    String = 2,
    Integer = 3,
    Boolean = 4,
    Double = 5,
}

/// A string returned by the graph DB as a vector of bytes
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphString(pub Vec<u8>);

// Methods to round-trip between regular strings and GraphStrings

impl From<String> for GraphString {
    fn from(string: String) -> Self {
        Self(string.into_bytes())
    }
}

impl From<Vec<u8>> for GraphString {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<GraphString> for Vec<u8> {
    fn from(redis_string: GraphString) -> Self {
        redis_string.0
    }
}

// A node returned by the Graph DB provider
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The labels attached to this node.
    pub labels: Vec<String>,
    /// The properties of this node.
    pub properties: HashMap<String, Scalar>,
}

// Macro generates generic "From" implementations to allow
// tuples/vecs-of-tuples to be extracted from various types
//
// Altered version of https://github.com/mitsuhiko/redis-rs/blob/master/src/types.rs#L1080
macro_rules! impl_row_for_tuple {
    () => ();
    ($($name:ident,)+) => (
        #[doc(hidden)]
        impl<$($name: FromCell),*> FromRow for ($($name,)*) {
            // we have local variables named T1 as dummies and those
            // variables are unused.
            #[allow(non_snake_case, unused_variables, clippy::eval_order_dependence)]
            fn from_row(result_set: &ResultSet, row_idx: usize) -> GraphResult<($($name,)*)> {
                // hacky way to count the tuple size
                let mut n = 0;
                $(let $name = (); n += 1;)*
                if result_set.num_columns() != n {
                    return client_type_error!(
                        "failed to construct tuple: tuple has {:?} entries but result table has {:?} columns",
                        n,
                        result_set.num_columns()
                    );
                }

                // this is pretty ugly too. The { i += 1; i - 1 } is rust's
                // postfix increment :)
                let mut i = 0;
                Ok(($({let $name = (); $name::from_cell(result_set, row_idx, { i += 1; i - 1 })?},)*))
            }
        }
        impl_row_for_tuple_peel!($($name,)*);
    )
}

// Support for the recursive macro calls
macro_rules! impl_row_for_tuple_peel {
    ($name:ident, $($other:ident,)*) => (impl_row_for_tuple!($($other,)*);)
}

// The library supports tuples of up to 12 items
impl_row_for_tuple! { T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, }

// Row and column indices default to zero for lower-level values
impl<T: FromCell> FromRow for T {
    fn from_row(result_set: &ResultSet, row_idx: usize) -> GraphResult<Self> {
        T::from_cell(result_set, row_idx, 0)
    }
}

impl<T: FromRow> FromTable for T {
    fn from_table(result_set: &ResultSet) -> GraphResult<Self> {
        T::from_row(result_set, 0)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // Verifies that we can extract the tuples we expect from the raw ResultSet
    // structure and that the various return types are automatically converted
    #[test]
    fn tuple_extraction_test() {
        let (name, birth_year): (String, u32) = fake_query("fake query").unwrap();
        assert_eq!("tester", name);
        assert_eq!(1985, birth_year);
    }

    #[test]
    fn vec_tuple_extraction_test() {
        let res: Vec<(String, u32)> = fake_vec_query("foo").unwrap();
        assert_eq!(("tester".to_string(), 1985), res[0]);
        assert_eq!(("test2".to_string(), 1986), res[1]);
    }

    fn fake_vec_query<T: FromTable>(_query: &str) -> GraphResult<T> {
        query_with_statistics2().map(|(value, _)| value)
    }

    fn fake_query<T: FromTable>(_query: &str) -> GraphResult<T> {
        query_with_statistics().map(|(value, _)| value)
    }

    fn query_with_statistics<T: FromTable>() -> GraphResult<(T, Statistics)> {
        let result_set = get_result_set()?;
        let value = T::from_table(&result_set)?;
        Ok((value, result_set.statistics))
    }

    fn query_with_statistics2<T: FromTable>() -> GraphResult<(T, Statistics)> {
        let result_set = get_result_set2()?;
        let value = T::from_table(&result_set)?;
        Ok((value, result_set.statistics))
    }

    fn get_result_set() -> GraphResult<ResultSet> {
        Ok(ResultSet {
            statistics: Statistics(vec![]),
            columns: vec![
                Column::Scalars(vec![Scalar::String(GraphString::from(
                    "tester".to_string(),
                ))]),
                Column::Scalars(vec![Scalar::Integer(1985)]),
            ],
        })
    }

    fn get_result_set2() -> GraphResult<ResultSet> {
        Ok(ResultSet {
            statistics: Statistics(vec![]),
            columns: vec![
                Column::Scalars(vec![
                    Scalar::String(GraphString::from("tester".to_string())),
                    Scalar::String(GraphString::from("test2".to_string())),
                ]),
                Column::Scalars(vec![Scalar::Integer(1985), Scalar::Integer(1986)]),
            ],
        })
    }
}
