namespace org.wasmcloud.core.v0

// Note: this file has moved into the wasmcloud/models crate

use org.wasmcloud.model.v0#nonEmptyString

/// a protocol defines the semantics
/// of how a client and server communicate.
@protocolDefinition
@trait(selector: "service")
structure wapc {}


/// Capability contract id, e.g. 'wasmcloud:httpserver'
@nonEmptyString
string CapabilityContractId


/// The `capability` trait indicates that the api is part of a
/// capability provider contract. A `capability` api may be
/// in either direction: actor-to-provider (providerReceiver),
/// or provider-to-actor (actorReceiver).
@trait(selector: "service")
structure capability {
  @required
  contractId: CapabilityContractId
}

/// the providerReceiver trait indicates service messages handled by a
/// capability provider (actor-to-provider).
@trait(selector: "service")
structure providerReceiver { }

/// the actorReceiver trait indicates service messages handled
/// by an actor (either actor-to-actor or provider-to-actor).
@trait(selector: "service")
structure actorReceiver { }

/// Actor service
@actorReceiver
@wapc
service Actor {
  version: "0.1",
  operations: [ HealthRequest ]
}

/// Capability Provider messages received from host
@providerReceiver
@wapc
service CapabilityProvider {
  version: "0.1",
  operations: [ BindActor, RemoveActor, HealthRequest ]
}

/// instruction to capability provider to bind actor
operation BindActor {
    input: String
}

/// instruction to capability provider to remove actor actor
operation RemoveActor {
    input: String
}

/// Return value from actors and providers for health check status
structure HealthCheckResponse {

  /// A flag that indicates the the actor is healthy
  healthy: Boolean

  /// A message containing additional information about the actors health
  message: String
}

/// health check request parameter
structure HealthCheckRequest { }

/// Perform health check. Called at regular intervals by host
operation HealthRequest {
    input: HealthCheckRequest
    output: HealthCheckResponse
}
