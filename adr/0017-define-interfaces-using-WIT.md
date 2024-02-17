# Define Interfaces using WIT

| Status   | Deciders                                                                | Date        |
|----------|-------------------------------------------------------------------------|-------------|
| Accepted | Commenters on [#336](https://github.com/wasmCloud/wasmCloud/issues/336) | 16 Feb 2024 |

## Context and Problem Statement

To enable distributed communication and RPC behavior on wasmCloud lattices, [AWS Smithy][smithy] was chosen as a [Interface Definition Language (IDL)][wiki-idl].

As Smithy is protocol-agnostic and does not define *how* to serialize or deserialize the data, the combination of Smithy and [MessagePack][msgpack] is what makes up [`wasmbus`][wasmbus], the primary means of communication between hosts, providers, and actors on wasmCloud lattices.

<details>
<summary><h3><code>wasmcloud:keyvalue</code> Smithy contract excerpt</h3></summary>

```smithy
// key-value.smithy
// Definition of a key-value store and the 'wasmcloud:keyvalue' capability contract
//

// Tell the code generator how to reference symbols defined in this namespace
metadata package = [{
    namespace: "org.wasmcloud.interface.keyvalue",
    crate: "wasmcloud_interface_keyvalue",
    py_module: "wasmcloud_interface_keyvalue",
    doc: "Keyvalue: wasmcloud capability contract for key-value store",
}]

namespace org.wasmcloud.interface.keyvalue

use org.wasmcloud.model#wasmbus
use org.wasmcloud.model#rename
use org.wasmcloud.model#n
use org.wasmcloud.model#U32
use org.wasmcloud.model#I32

@wasmbus(
    contractId: "wasmcloud:keyvalue",
    providerReceive: true )
service KeyValue {
  version: "0.1.1",
  operations: [
    Increment, Contains, Del, Get,
    ListAdd, ListClear, ListDel, ListRange,
    Set, , SetAdd, SetDel, SetIntersection, SetQuery, SetUnion, SetClear,
  ]
}

/// Gets a value for a specified key. If the key exists,
/// the return structure contains exists: true and the value,
/// otherwise the return structure contains exists == false.
@readonly
operation Get {
  input: String,
  output: GetResponse,
}

/// Response to get request
structure GetResponse {
    /// the value, if it existed
    @required
    @n(0)
    value: String,
    /// whether or not the value existed
    @required
    @n(1)
    exists: Boolean,
}

// ...
```

</details>

The combination that is `wasmbus` offered us a few benefits:

- Schema definition
- Code generation (given use of the schema)
- Structured serialization and deserialization of operations & data

As the WebAssembly ecosystem has matured, various components have come together to offer a WebAssembly-native solution to the problem of interoperability between WebAssembly modules (and components):

- The [Component Model][cm] defined a consistent way for WebAssembly modules & components to interact
- [WebAssembly Interface Types][wit] defined an IDL for operations between modules & components

<details>
<summary><h3><code>wasmcloud:keyvalue</code> WIT contract excerpt</h3></summary>

```wit
package wasmcloud:keyvalue;

// Based on https://github.com/wasmCloud/interfaces/blob/f020c93d4cacd50318301f686e2f059a15862e1e/keyvalue/keyvalue.smithy

interface key-value {
    record get-response {
        value: string,
        exists: bool,
    }

    get: func(input: string) -> get-response;

    // ...
}
```

</details>

After much iteration and hard work done by the [Bytecode Alliance][bca] and other contributors, WIT has emerged as a well designed IDL, with a growing ecosystem of code generators as well.

Rather than continue to use Smithy or define a Smithy to WIT bridge, we find it important to be fully aligned with the open source ecosystem in prioritizing use of WIT as our means of schema definition and code generation. While structured serialization and deserialization is *not* a part of WIT natively, those concerns are somewhat tangential to the change (i.e. theoretically MessagePack could be reused).

In becoming "WIT-first", we "WIT-ified" all of the major pieces and components of wasmCloud:

- The host was WIT-ified such that it could support invocations derived from WIT contracts
- Providers were WIT-ified such that they could generate code that
- Actors were converted to use available bindgen primitives from upstream open source

[smithy]: https://smithy.io
[wiki-idl]: https://en.wikipedia.org/wiki/Interface_description_language
[bca]: https://bytecodealliance.org/
[cm]: https://github.com/WebAssembly/component-model
[wit]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
[msgpack]: https://msgpack.org/
[wasmbus]: https://docs.rs/wasmbus-rpc/latest/wasmbus_rpc

## Decision Drivers <!-- optional -->

* Desire to contribute to the upstream open source efforts
* Desire to build on a common, well-designed base of IDL functionality for WebAssembly

## Considered Options

* Converting to "WIT-first" stance with support across host, providers, and actors
* Continuing to use Smithy
* Building Smithy-to-WIT bridging tooling

## Decision Outcome

Chosen option: "Converting to "WIT-first" stance with support across host, providers, and actors", because going forward, most of the ecosystem will be pulling in the same direction, and attempting to build against or without regards for standards makes wasmCloud less interoperable and the ecosystem as a whole weaker as a result.

### Positive Consequences

* Ability to interoperate with other ecosystem projects
* Better cross-language support as the ecosystem grows
* Better general support for typing (WIT mirrors Rust's type system much more than Smithy and has many useful native constructs)
* Ability to test and demonstrate latest bleeding-edge WebAssembly & WIT functionality, contributing positively to the WebAssembly ecosystem

### Negative Consequences

* Team training required to understand and use WIT
* Exposure to upstream instability/breaking changes and uncertainty
* Additional codebase complexity as a result of supporting both `wasmbus` and WIT

## Links

* [Original RFC #336](https://github.com/wasmCloud/wasmCloud/issues/336)
