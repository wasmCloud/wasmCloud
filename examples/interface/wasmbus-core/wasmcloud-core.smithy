metadata package = [ { namespace: "org.wasmcloud.core", crate: "wasmbus_rpc::core" } ]
namespace org.wasmcloud.core

use org.wasmcloud.model#nonEmptyString
use org.wasmcloud.model#codegenRust
use org.wasmcloud.model#serialization
use org.wasmcloud.model#CapabilityContractId

/// data sent via wasmbus
@trait(selector: "structure")
@codegenRust( deriveDefault: true )
structure wasmbusData {}

/// Actor service
@wasmbus(
    actorReceive: true,
)
service Actor {
  version: "0.1",
  operations: [ HealthRequest ]
}


/// instruction to capability provider to bind actor
@idempotent
operation PutLink {
    input: LinkDefinition
}

/// instruction to capability provider to remove actor
@idempotent
operation DeleteLink {
    input: String
}

/// Returns list of all actor links for this provider
@readonly
operation GetLinks {
    output: ActorLinks
}

/// Returns true if the link is defined
@readonly
operation HasLink {
    input: String,
    output: Boolean,
}

/// Link definition for an actor
@wasmbusData
structure LinkDefinition {
    /// actor public key
    @required
    @serialization(name:"actor_id")
    actorId: String,

    /// provider public key
    @required
    @serialization(name:"provider_id")
    providerId: String,

    /// link name
    @required
    @serialization(name:"link_name")
    linkName: String,

    /// contract id
    @required
    @serialization(name:"contract_id")
    contractId: String,

    @required
    values: LinkSettings,
}


/// Return value from actors and providers for health check status
@wasmbusData
structure HealthCheckResponse {

  /// A flag that indicates the the actor is healthy
  healthy: Boolean

  /// A message containing additional information about the actors health
  message: String
}

/// health check request parameter
@wasmbusData
structure HealthCheckRequest { }

/// Perform health check. Called at regular intervals by host
operation HealthRequest {
    input: HealthCheckRequest
    output: HealthCheckResponse
}

/// Settings associated with an actor-provider link
map LinkSettings {
    key: String,
    value: String,
}

/// List of linked actors for a provider
list ActorLinks {
    member: LinkDefinition
}

/// The response to an invocation
structure HostData {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "lattice_rpc_prefix")
    latticeRpcPrefix: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @required
    @serialization(name: "lattice_rpc_user_jwt")
    latticeRpcUserJwt: String,

    @required
    @serialization(name: "lattice_rpc_user_seed")
    latticeRpcUserSeed: String,

    @required
    @serialization(name: "lattice_rpc_url")
    latticeRpcUrl: String,

    @required
    @serialization(name: "provider_key")
    providerKey: String,

    @required
    @serialization(name: "env_values")
    envValues: HostEnvValues,
}

map HostEnvValues {
    key: String,
    value: String,
}

/// RPC message to capability provider
structure Invocation {
    @required
    origin: WasmCloudEntity,

    @required
    target: WasmCloudEntity,

    @required
    operation: String,

    @required
    msg: Blob,

    @required
    id: String,

    @required
    @serialization(name: "encoded_class")
    encodedClass: String,

    @required
    @serialization(name: "host_id")
    hostId: String,
}

structure WasmCloudEntity {

    @required
    @serialization(name: "public_key")
    publicKey: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @required
    @serialization(name: "contract_id")
    contractId: CapabilityContractId,
}


