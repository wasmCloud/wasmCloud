namespace org.wasmcloud.example.httpServer
// interface for a simple http server

use org.wasmcloud.core#actorReceiver
use org.wasmcloud.core#CapabilityContractId
use org.wasmcloud.core#capability
use org.wasmcloud.model#U32

/// HttpServer is the contract to be implemented by actor
@actorReceiver
@capability(contractId: "wasmcloud:httpserver")
service HttpServer {
  version: "0.1",
  operations: [ HandleRequest ]
}

/// HttpRequest contains data sent to actor about the http request
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

operation HandleRequest {
  input: HttpRequest,
  output: HttpResponse,
}

/// Headers is a list of http headers
map Headers {
  key: String,
  value: String,
}

