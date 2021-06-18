// interface for lattice control

namespace org.wasmcloud.lattice

use org.wasmcloud.model#U64
use org.wasmcloud.model#serialize
use org.wasmcloud.core#CapabilityContractId

/// the controlApi trait indicates control messages to a host.
/// Control messages are sent over NATS
@trait(selector: "service")
structure controlApi { }

@controlApi
service LatticeControl {
  version: "0.1",
  operations: [
    PerformInvocation,
    PutLinkDefinition,
    DelLinkDefinition,
    PutClaims,
    GetClaims,
    PutActorReference,
    PutProviderReference,
 ],
}

structure ClaimsHeader {

    @serialize(rename: "typ")
    headerType: String,

    @serialize(rename: "alg")
    algorithm: String,
}

structure CapabilityProvider {
    /// descriptive name for the capability provider
    @required
    name: String,

    /// capability contract id this provider supports
    @required
    capid: CapabilityContractId,

    /// A human-readable string identifying the vendor of this provider
    /// Examples: Redis, Cassandra, NATS
    @required
    vendor: String,

    /// monotonically increasing revision number. Optional.
    rev: String

    /// human-friendly version string. Optional
    ver: String,

    /// file hashes that correspond to the architecture-OS target triples for this
    /// provider
    targetHashes: TargetTriples,

}

/// file hashes corresponding to the architecture-OS target triples for this provider
map TargetTriples {
    key: String,
    value: String,
}

/// Represents a set of [RFC 7519](https://tools.ietf.org/html/rfc7519) compliant JSON Web Token
/// claims.
structure Claims {

    /// All timestamps in JWTs are stored in _seconds since the epoch_ format
    /// as described as `NumericDate` in the RFC. Corresponds to the `exp` field in a JWT.
    /// Optional.
    @serialize(rename: "exp")
    expires: U64,

    /// Corresponds to the `jti` field in a JWT.
    @serialize(rename: "jti")
    id: String,

    /// The `iat` field, stored in _seconds since the epoch_
    @serialize(rename: "iat")
    issuedAt: U64,

    /// Issuer of the token, by convention usually the public key of the _account_ that
    /// signed the token
    @serialize(rename: "iss")
    issuer: String,

    /// Subject of the token, usually the public key of the _module_ corresponding to the WebAssembly file
    /// being signed
    @serialize(rename: "sub")
    subject: String,

    /// The `nbf` JWT field, indicates the time when the token becomes valid. If `None` token is valid immediately
    @serialize(rename: "nbf")
    notBefore: U64,

    /// Optional custom jwt claims in the `wascap` namespace
    @serialize(rename: "wascap")
    metadata: Blob,
}

list ClaimsList {
    member: Claims,
}

operation PerformInvocation {
  input: InvocationArgs,
  output: InvocationResponse,
}

// TODO: how to indicate status?
// presence of response is not sufficient - successful invocation with void response
// would appear same as error response
structure InvocationResponse {
  /// response - The response, if successful
  response: Blob,
}

/// Performs an invocation that can either target an actor or a capability provider.
/// It is up to the caller to ensure that all of the various arguments make sense for this
/// invocation, otherwise it will fail.
structure InvocationArgs {
  /// actorKey - The public key of the actor performing the invocation. If the invocation is coming from a provider, this argument is ignored.
  actorKey: String,

  /// binding - The link name for the invocation. This will be ignored in actor-to-actor calls.
  binding: String,

  /// namespace - The namespace of the operation. For provider calls, this will be something like `wasmcloud:messaging`. For actor targets, this is the public key of the target actor
  @required
  namespace: String,

  /// payload - The raw bytes of the payload to be placed _inside_ the invocation envelope. Do **not** serialize an invocation for this parameter, one is created for you.
  @required
  payload: Blob,

  /// seed - The seed signing key of the host, used for invocation anti-forgery tokens.
  @required
  seed: String,

  /// prefix - The lattice subject prefix of the lattice into which this invocation is being sent.
  @required
  prefix: String,

  /// provider_key - The public key of the capability provider involved in this invocation. This value will be ignored for actor targets.
  providerKey: String,
}

/// Publishes a link definition to the lattice for both caching and processing. Any capability provider
/// with the matching identity triple (key, contract, and link name) will process the link definition idempotently. If
/// the definition exists, nothing new will happen.
///
/// This function does not return a success indicator for the link processing, only an indicator for
/// whether the link definition publication was successful.
operation PutLinkDefinition {
  input: PutLinkArgs,
  output: InvocationResponse,
}

/// Parameters for putLinkDefinition
structure PutLinkArgs {

  /// actorKey - The public key of the actor to be linked
  actorKey: String,

  /// providerKey - public key of capability provider
  providerKey: String,

  /// linkName - name of link used when target provider was loaaded
  linkName: String,

  /// contractId - capability provider contract id
  contractId: CapabilityContractId,

  /// prefix - lattice namespace prefix
  prefix: String,

  /// binding - The link name for the invocation. This will be ignored in actor-to-actor calls.
  values: LinkBindingParams,
}

/// Key-Value pairs for binding capability provider
map LinkBindingParams {
  key: String,
  value: String,
}

/// Removes a link definition from the cache and tells the appropriate provider to
/// de-activate that link and remove andy resources associated with that link name
/// from the indicated actor.
operation DelLinkDefinition {
  input: DelLinkArgs,
  output: DelLinkResponse,
}

structure DelLinkArgs {

  /// actorKey - The public key of the linked actor
  actorKey: String,

  /// providerKey - public key of capability provider
  providerKey: String,

  /// linkName - name of link used when target provider was loaaded
  linkName: String,

  /// contractId - capability provider contract id
  contractId: CapabilityContractId,

  /// prefix - lattice namespace prefix
  prefix: String,
}

structure DelLinkResponse {}

/// Queries the list of link definitions that are active and applied to the indicated
/// capability provider. This query is done on a queue group topic so if the provider
/// is horizontally scaled, you will still only get a single response.
operation GetLinkDefinitions {
    input: GetLinkArgs,
    output: LinkDefinitions,
}

structure GetLinkDefinitions {

    /// prefix - Lattice namespace prefix
    prefix: String,

    /// provider_key - Public key of the capability provider
    providerKey: String,

    /// link_name - Link name of the capability provider
    linkName: String,

    /// Link parameters
    linkParams: LinkBindingParams,
}

structure LinkDefinition {

    actorId: String,    // todo: change to actorKey
    providerId: String, // todo: change to providerKey
    linkName: String,
    contractId: CapabilityContractId,
    values: LinkBindingParams,
}

list LinkDefinitions {
    member: LinkDefinition,
}

/// Publishes a set of claims for a given entity. The claims will be cached by all
/// listening participants in the lattice where applicable.
operation PutClaims {
  input: PutClaimsArgs,
  // output: String, // TODO: void
}

/// Parameters for putLinkDefinition
structure PutClaimsArgs {

  /// prefix - lattice namespace prefix
  prefix: String,

  /// claims - claims to be published
  claims: Claims,
}

/// Queries the distributed cache for all known claims. Note that this list does not
/// auto-purge when actors and providers are de-scheduled, claims must be manually removed.
/// As such, it's likely that this list will contain claims for entities that are no
/// longer up and running. This is as designed.
operation GetClaims {
    // prefix - lattice namespace prefix
    input: String,
    output: ClaimsList,
}

/// Publishes a reference from an alias (OCI or call alias) to a given WasmCloudEntity. This
/// reference will be added to the distributed cache and made available to all hosts for
/// quick lookup. References will be checked before invocations and other administrative
/// operations wherever possible, falling back on public keys when no reference is found.
operation PutActorReference {
    input: PutActorReferenceArgs,
    // output: String, // TODO: void
}

/// Publishes a reference from an alias (OCI or call alias) to a capability provider.
/// reference will be added to the distributed cache and made available to all hosts for
/// quick lookup. References will be checked before invocations and other administrative
/// operations wherever possible, falling back on public keys when no reference is found.
operation PutProviderReference {
    input: PutProviderReferenceArgs,
    // output: String, // TODO: void
}

structure PutActorReferenceArgs {
    /// prefix - lattice namespace prefix
    prefix: String,
    /// source- reference to be added (OCI call alias)
    /// either oci reference or callAlias
    source: String
    /// actor target of the reference
    target: String,
}

structure PutProviderReferenceArgs {
    /// prefix - lattice namespace prefix
    prefix: String,

    /// source- reference to be added (OCI call alias)
    /// either oci reference or callAlias
    source: String

    /// capability provider target
    target: Capability,
}

structure Capability {
    actorKey: String,
    contractId: CapabilityContractId,
    linkName: String,
}
