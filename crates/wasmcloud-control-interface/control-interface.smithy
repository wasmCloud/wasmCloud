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
    host_id: String,

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
    image_ref: String,

    name: String,

    @required
    revision: I32,
}

structure ProviderDescription {

    @required
    id: String,

    @required
    @serialization(name: "link_name")
    link_name: String,

    @serialization(name: "image_ref")
    image_ref: String,

    name: String,

    @required
    revision: I32,
}


structure StartActorCommand {
    @required
    actorRef: String,

    @required
    @serialization(name: "host_id")
    hostId: String,
}

structure StartActorAck {
    @required
    @serialization(name: "actor_ref")
    actorRef: String,

    @required
    @serialization(name: "host_id")
    hostId: String,

    /// optional failure message
    failure: String,
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

structure StartProviderAck {
    @required
    @serialization(name: "host_id")
    hostId: String,

    @required
    @serialization(name: "provider_ref")
    providerRef: String,

    /// optional failure message
    failure: String,
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

structure StopActorAck {
    failure: String,
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

structure StopProviderAck {
    failure: String,
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

structure UpdateActorAck {
    @required
    accepted: Boolean,
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
    uptime: U64
}

list HostList {
    member: Host,
}


/// response to get_claims
structure GetClaimsResponse {
    @required
    claims: ClaimsList
}

structure Claims {
    @required
    values: ClaimsMap,
}

list ClaimsList {
    member: Claims,
}

map ClaimsMap {
    key: String,
    value: String,
}
