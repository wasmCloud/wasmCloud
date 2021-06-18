namespace org.wasmcloud.core

/// Capability contract id, e.g. 'wasmcloud:httpserver'
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


/// Capability Provider messages received from host
@providerReceiver
service CapabilityProvider {
  version: "0.1",
  operations: [ BindActor, RemoveActor ]
}

/// instruction to capability provider to bind actor
operation BindActor {
    input: String
}

/// instruction to capability provider to remove actor actor
operation RemoveActor {
    input: String
}
