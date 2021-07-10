metadata links = [
    {
        // doc links for information generator
        namespace: "org.wasmcloud.example.rangekv",
        doc_url: "https://wasmcloud.github.io/models/org_wasmcloud_example_rangekv.html",
    }
]

namespace org.wasmcloud.example.rangekv
// a key-value store with sorted keys and pagination

// This key-value store demonstrates some smithy features:
// - traits: paginated, length, idempotent, readonly
// - functions returning void
// - functions with no parameters

use org.wasmcloud.core#wasmbus
use org.wasmcloud.model#U32
use org.wasmcloud.model#U64

@wasmbus(
    contractId: "wasmcloud::example:rangekv",
    providerReceive: true,
)
service RangeKeyValue{
  version: "0.1",
  operations: [ Get, Put, Delete, Clear, Contains, Keys, Values, Size  ]
}

/// Gets a value for a specified key.
@readonly
operation Get {
  input: Key,
  output: MaybeValue
}

/// Stores a value. Replaces an existing value of the same key.
@idempotent
operation Put {
  input: KeyValue,
}

/// Structure that contains an optional value
structure MaybeValue {
  /// a value or none
  value: BlobValue,
}

/// A structure containing a key and value
structure KeyValue {
  @required
  key: Key,
  @required
  value: BlobValue,
}

/// Deletes a value if it exists.
@idempotent
operation Delete {
  input: Key,
}

/// Clears all keys
@idempotent
operation Clear { }


/// Returns true if the value is contained in the store
@readonly
operation Contains {
  input: Key,
  output: Boolean
}

/// Returns a range of keys
@paginated(
    inputToken: "startKey",
    outputToken: "nextKey",
    items: "items",
    pageSize: "limit",
)
@readonly
operation Keys {
    input: RangeRequest,
    output: KeyRangeResponse,
}

/// Returns a range of key-value pairs
@paginated(
    inputToken: "startKey",
    outputToken: "nextKey",
    items: "items",
    pageSize: "limit",
)
@readonly
operation Values {
    input: RangeRequest,
    output: KeyValueRangeResponse,
}


/// Input a range request (Keys or Values)
structure RangeRequest {
    /// the initial key at start of range
    @required
    startKey: String,

    /// optional last key of the requested range (inclusive)
    lastKey: String,

    /// maximum number of values to return
    /// the server may return fewer than this value.
    @required
    limit: U32,
}

structure KeyRangeResponse {
    /// first key in range returned
    @required
    startKey: String,

    /// startKey that should be used on the next request
    /// If this value is empty, there are no more keys
    nextKey: String,

    /// number of items returned
    @required
    count: U32,

    /// values returned
    @required
    items: KeyList,
}

/// result of Values range query
structure KeyValueRangeResponse {
    /// first key in range returned
    @required
    startKey: String,

    /// startKey that should be used on the next request
    /// If this value is empty, there are no more keys
    nextKey: String,

    /// number of items returned
    @required
    count: U32,

    /// returned list of key-value pairs
    @required
    items: KeyValueList,
}

/// Returns the number of items in the store
@readonly
operation Size {
    output: U64,
}

/// A list of keys
list KeyList {
    member: Key,
}

/// A list of key-value pairs
list KeyValueList {
    member: KeyValue,
}

/// Key is any non-empty UTF-8 string
@length(min:1)
string Key


/// BlobValue is a non-empty byte array
@length(min:1)
blob BlobValue