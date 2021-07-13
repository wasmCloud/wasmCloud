// hello.smithy
// Definition for a simple hello-world responder
//

// Tell the code generator how to reference symbols defined in this namespace
metadata package = [ { namespace: "org.wasmcloud.example.hello", crate: "wasmcloud_example_hello" } ]

namespace org.wasmcloud.example.hello

use org.wasmcloud.core#wasmbus
use org.wasmcloud.model#U32
use org.wasmcloud.model#U64

/// Simple service that responds to a message
@wasmbus(
    contractId: "wasmcloud:example:hello",
    actorReceive: true,
    providerReceive: true )
service Hello {
  version: "0.1",
  operations: [ sayHello  ]
}

/// Send a string message
///.Response is "Hello " + input message
@readonly
operation SayHello {
  input: String,
  output: String
}