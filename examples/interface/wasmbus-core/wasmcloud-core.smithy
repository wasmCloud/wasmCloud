namespace org.wasmcloud.core

use org.wasmcloud.model#nonEmptyString
use org.wasmcloud.model#codegenRust
use org.wasmcloud.model#serialization

/// a protocol defines the semantics
/// of how a client and server communicate.
@protocolDefinition
@trait(selector: "service")
structure wasmbus {
    /// capability id such as "wasmbus:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    contractId: CapabilityContractId,
    /// indicates this service's operations are handled by an actor (default false)
    actorReceive: Boolean,
    /// indicates this service's operations are handled by an provider (default false)
    providerReceive: Boolean,
}

/// data sent via wasmbus
@trait(selector: "structure")
@codegenRust( deriveDefault: true )
structure wasmbusData {}


/// Capability contract id, e.g. 'wasmcloud:httpserver'
@nonEmptyString
string CapabilityContractId


/// Actor service
@wasmbus(
    actorReceive: true,
)
service Actor {
  version: "0.1",
  operations: [ HealthRequest ]
}

/// CapabilityProvider service handles link + health-check messages from host
/// (need to finalize Link apis)
@wasmbus(
    providerReceive: true,
)
service CapabilityProvider {
  version: "0.1",
  operations: [ HealthRequest ]

  //operations: [ PutLink, DeleteLink, GetLinks, HasLink, HealthRequest ]
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