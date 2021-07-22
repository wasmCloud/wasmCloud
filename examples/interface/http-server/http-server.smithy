// http-server.smithy
// Definition of the wasmbus:httpserver capability contract
//

// Tell the code generator how to reference symbols defined in this namespace
metadata package = [ { namespace: "org.wasmcloud.example.httpServer", crate: "wasmcloud_example_httpserver" } ]

namespace org.wasmcloud.example.httpServer

use org.wasmcloud.model#codegenRust
use org.wasmcloud.model#U32
use org.wasmcloud.model#wasmbus
use org.wasmcloud.model#wasmbusData

/// HttpServer is the contract to be implemented by actor
@wasmbus(
    contractId: "wasmcloud:httpserver",
    actorReceive: true,
)
service HttpServer {
  version: "0.1",
  operations: [ HandleRequest ]
}

operation HandleRequest {
  input: HttpRequest,
  output: HttpResponse,
}

/// HttpRequest contains data sent to actor about the http request
@wasmbusData
structure HttpRequest {
  /// HTTP method. One of: GET,POST,PUT,DELETE,HEAD,OPTIONS,CONNECT,PATCH,TRACE
  @required
  method: String,
  /// full request path
  @required
  path: String,
  /// query string. May be an empty string if there were no query parameters.
  @required
  queryString: String,
  /// map of request headers (string key, string value)
  @required
  header: Headers,
  /// Request body as a byte array. May be empty.
  @required
  body: Blob,
}

/// HttpResponse contains the actor's response to return to the http client
@wasmbusData
// don't generate Default since we want to customize it
@codegenRust( deriveDefault: false )
structure HttpResponse {
  /// statusCode is a three-digit number, usually in the range 100-599,
  /// A value of 200 indicates success.
  @required
  statusCode: U32,
  /// status response, usually "OK"
  @required
  status: String,
  /// Map of headers (string keys, string values)
  @required
  header: Headers,
  /// Body of response as a byte array. May be an empty array.
  @required
  body: Blob,
}

/// Headers is a list of http headers
map Headers {
  key: String,
  value: String,
}
