// control-interface.smithy
//
// host control interface
//

// Tell the code generator how to reference symbols defined in this namespace
metadata package = [
    {
        namespace: "org.wasmcloud.interface.control",
        crate: "wasmcloud-control-interface"
     }
]

namespace org.wasmcloud.interface.control

use org.wasmcloud.model#serialization
use org.wasmcloud.core#ActorLinks
use org.wasmcloud.model#I32
use org.wasmcloud.model#U16
use org.wasmcloud.model#U64


structure ProviderAuctionRequest {

    @required
    @serialization(name: "provider_ref")
    providerRef: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @required
    constraints: ConstraintMap,
}

map ConstraintMap {
    key: String,
    value: String,
}

structure ProviderAuctionAck {
    @required
    @serialization(name: "provider_ref")
    providerRef: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @required
    @serialization(name: "host_id")
    hostId: String,
}

structure ActorAuctionRequest {
    @required
    @serialization(name: "actor_ref")
    actorRef: String,

    @required
    constraints: ConstraintMap,
}

structure ActorAuctionAck {
    @required
    @serialization(name: "actor_ref")
    actorRef: String,

    @required
    constraints: ConstraintMap,

    @required
    @serialization(name: "host_id")
    hostId: String,
}

structure HostInventory {

    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    labels: LabelsMap,

    @required
    actors: ActorDescriptions,

    @required
    providers: ProviderDescriptions,
}

map LabelsMap {
    key: String,
    value: String,
}

list ActorDescriptions {
    member: ActorDescription,
}

list ProviderDescriptions {
    member: ProviderDescription,
}

structure ActorDescription {

    @required
    id: String,

    @serialization(name: "image_ref")
    imageRef: String,

    name: String,

    @required
    revision: I32,
}

structure ProviderDescription {

    @required
    id: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @serialization(name: "image_ref")
    imageRef: String,

    name: String,

    @required
    revision: I32,
}


structure StartActorCommand {
    @required
    @serialization(name: "actor_ref")
    actorRef: String,

    @required
    @serialization(name: "host_id")
    hostId: String,
}

structure StartProviderCommand {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "provider_ref")
    providerRef: String,

    @required
    @serialization(name: "link_name")
    linkName: String,
}

structure StopActorCommand {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "actor_ref")
    actorRef: String,

    /// optional count
    count: U16,
}

structure StopProviderCommand {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "provider_ref")
    providerRef: String,

    @required
    @serialization(name: "link_name")
    linkName: String,

    @required
    @serialization(name: "contract_id")
    contractId: String,
}

structure UpdateActorCommand {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "actor_id")
    actorId: String,

    @required
    @serialization(name: "new_actor_ref")
    newActorRef: String,
}

/// Standard response for control interface operations
structure CtlOperationAck {
    @required
    accepted: Boolean,
    @required
    error: String
}

structure LinkDefinitionList {
    @required
    links: ActorLinks
}

structure Host {
    @required
    id: String,

    /// uptime in seconds
    @required
    @serialization(name: "uptime_seconds")
    uptimeSeconds: U64
}

list HostList {
    member: Host,
}

/// response to get_claims
structure GetClaimsResponse {
    @required
    claims: ClaimsList
}

list ClaimsList {
    member: ClaimsMap,
}

map ClaimsMap {
    key: String,
    value: String,
}
