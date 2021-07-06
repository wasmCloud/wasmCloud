namespace org.wasmcloud.example.hello

use org.wasmcloud.core#capability
use org.wasmcloud.model#U32
use org.wasmcloud.model#U64

/// Simple service that responds to a hello message
@capability(contractId: "wasmcloud:example:hello")
@actorReceiver
@providerReceiver
service Hello {
  version: "0.1",
  operations: [ sayHello  ]
}

/// Send a hello message
@readonly
operation SayHello {
  input: String,
  output: String
}