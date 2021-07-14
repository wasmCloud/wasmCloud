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
  @required
  method: String,
  @required
  path: String,
  @required
  queryString: String,
  @required
  header: Headers,
  @required
  body: Blob,
}

/// HttpResponse contains the actor's response to return to the http client
@wasmbusData
// don't generate Default since we want to customize it
@codegenRust( deriveDefault: false )
structure HttpResponse {
  /// statusCode should be 200 if the request was correctly handled
  @required
  statusCode: U32,
  @required
  status: String,
  @required
  header: Headers,
  @required
  body: Blob,
}


/// Headers is a list of http headers
map Headers {
  key: String,
  value: String,
}
