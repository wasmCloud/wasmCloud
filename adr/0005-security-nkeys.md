# Security Platform based on ed25519 and PKI

In this record we discuss the decision around the development of a flexible security platform based on Public Key Infrastructure (PKI) and specific support for ed25519 keys and a custom developer-friendly key encoding to accompany it.

## Context and Problem Statement

At the very core of wasmCloud's standalone and distributed modes (**lattice**) is the concept of _zero trust_ for participating entities. **Actors** within this ecosystem are not trusted and cannot be allowed to do anything for which they have not been granted explicit access, including the very basic act of starting/executing within a runtime host.

The next requirement above zero trust is that all of the metadata required to make security decisions about an entity must _accompany the entity_. Put another way, we are **not allowed** to make an external call to a single point of failure or some other form of remotely connected infrastructure in order to obtain the security profile of an actor. wasmCloud is designed from the very beginning to support edge workloads as well as workloads running in offline or partially connected environments. None of that is possible if we have to make a call to a server to ask it for the security credentials associated with a given actor or capability provider. wasmCloud is, and must remain, _decentralized_ while not sacrificing security.

Put bluntly, wasmCloud is a _decentralized_ system that mandates _decentralized_ security.

Further, wasmCloud is a _platform_. While you can run it out of the box for a great many use cases, it is expected that people will build products on top of wasmCloud. _Platform builders_ must be able to define their own unique security models using the tools available within wasmCloud. Beyond the certain core enforcements to assert that an entity's credentials are legitimate, not expired, and have not been tampered with, wasmCloud must allow platform builders to extend the security system however they see fit.

## Considered Options

Once we rule out the use of third-party sources for credentials and other security metadata, we're left with the notion of _embedding_. There are a number of different ways in which we can embed secure metadata within actor modules and _Provider Archive_ files. The following are the options we considered:

* x.509 certificates
* Signed JWTs with ed25519
* Proprietary Format and Verification

### x.509 Certificates

An x.509 certificate[^1] is a combination of a public key and a signature issued by a parent _certificate authority_ (CA). Each of these signing certificate authorities can in turn be verified by another authority. This linked list of authorities is called a _certificate chain_.

In this option, we would embed a certificate into a module and utilize the extensions facility of certificates to write certain types of metadata, which would have to include things like the list of capabilities granted and other wasmCloud-specific information.

The x.509 certificate file can have many different binary encodings[^2], each with their own use and pros and cons.

### Signed JWTs with ed25519

An x.509 certificate is a signed document. We can also sign other kinds of documents like JWTs (JSON Web Token)[^3] that have established standards for encoded string representation.

JSON Web Tokens (hereafter referred to simply as JWTs) are designed to carry _claims_. For more information on a _claim_, take a look at some of the available information on claims-based security and identity[^4]. There are standard claims that refer to the identity (`subject` or `sub`) and the verifying authority (`issuer` or `iss`), claims related to validity dates and expiration dates. Claims are represented internally as extensible JSON--the specification allows you to add your own arbitrary claims to any token so long as the required fields are there.

In this option we would embed a JWT (which is a set of 3 base64-encoded strings separated by a `.` character) into the module in question. Since the signature can be verified in place, this also meets our requirement of not requiring a third party source of data for the _core_ ("out of the box") functionality that ships with wasmCloud.

### Custom Encoding for ed25519 Keys (NKeys)

An add-on set of functionality that can be combined with the JWT option is the use of developer, operations, and user friendly key encoding. This encoding produces 56-character uppercase strings with a prefix indicating the role of the key (which is nothing more than a mnemonic and convention, there's nothing enforced about the role of a key). Seeds are 57-character strings prefixed with an `S` and then the role prefix of the key.

This encoding makes identities and keys usable by "regular humans" as well as by code without reducing the security of the keys (the raw binary version of these keys is 100% compatible with existing ed25519 tooling and libraries).

By design, this encoding can't be invalidated by changes to the encoding standard. wasmCloud supports more entity types than NATS (the inventor of this encoding) does, and yet another product could support either less or more. As mentioned elsewhere, this encoding doesn't impact the compatibility of keys with the ed25519 standard.

### Proprietary Format and Verification

This option is here for the sake of completeness. One possible solution to the problem would be to create our own metadata format, and our own method of securing said metadata. In this solution we would embed our own proprietary binary payload into zero-trust modules.

## Decision Outcome

In the end we chose to go with **signed JWTs** where the signature comes from a standard ed25519 private key (seed). We also chose to use a developer-and-log-friendly encoding that allows for keys to be "double-clickable", copyable, pastable, and have an easily identified "role" prefix.

The decision ultimately came down to ease of use, ease of expansion, and ease of "day-1" operations and maintenance. We must be ruthless in our drive for supporting the simplest and smoothest possible developer and operations experiences. JWTs and "simple string" keys "just work" and empower rather than hinder those who build on top of wasmCloud.

### Positive Consequences

* Meet our requirement of allowing platform builders the flexibility they need to create whatever solution they like on top of core tooling.
  * Platform builders can devise arbitrary hierarchies and allow for signing entities _to have multiple signatures_ creates enhanced security scenarios that limit blast radius of key exposure.
* Management of keys is extremely simple and low friction. The usefulness of the ability to scan for unique identities in logs and have "double-click" friendly keys in the developer workflow cannot be overstated.
* JWTs are "just JSON", and so our ability to extend and grow the security system over time can happen inside the JWT without violating standards or encodings.

### Negative Consequences

* Avoiding the use of x.509 certificates may give the (false) impression of us avoiding industry standards. The use of JWTs outside the realm of OAuth pipelines is not as well-publicized as the use of x.509 certs
* Not using x.509 certificates also means we cannot leverage the wealth of existing tools available for x.509-based infrastructure. (this also appears as a positive consequence, as we assert much of this maintenance infrastructure is high-complexity and high-friction).

## Pros and Cons of the Options

The following is a discussion of the benefits and drawbacks of the various solutions considered.

### x.509 Certificates

x.509 certificates certainly have the benefit of acceptance and use over time. They have been around for almost 3 decades in various forms, and so support for them is considered ubiquitous and available in practically every programming language.

However, management of certificates to be deployed in production environments for "chain verification" is nearly as universally considered to be high-friction, error-prone, and frustrating. While wasmCloud makes no judgement about _how_ a platform builder should construct their chains, we feel that forcing platform builders into the world of certificate installation, verification, and chain management foists too much of a complexity burden on them. Building a platform on top of wasmCloud should be _easy_, and _low-friction_, and we feel that cert management takes away from this ease of use.

Next, the use of metadata is not quite as flexible on these certs as it is with JWTs. x.509 certificates have a schema-fixed format, and if you want to support extension fields, you must do so in a way that conforms precisely to the applicable RFCs. This can present additional layers of friction and complexity when it comes to organic growth of what metadata resides inside the metadata embedded in wasmCloud actor and provider modules.

Further, certificate chains in this realm are necessarily vertical. There is less flexibility in how secure hierarchies can be created with x.509 certs than with arbitrary signers in the JWT scenario.

### JWTs with Custom Encoded ed25519 Keys

Using JWTs is a low-friction path. These documents are just base64-encoded strings, so the management of them is already an order of magnitude easier than x.509 binaries that support multiple different encodings.

The contents of a JWT have a few mandatory fields, but you are free to extend the `claims` contained within however you see fit, something that we will surely need as the capabilities of wasmCloud grow over time.

The signature of a JWT can be produced with any number of algorithms, but in this case we would use the `ed25519` algorithm. With the ability to validate that the issuer claim is the same as the public key of the entity that signed the JWT, we can validate a JWT _in isolation_, and then allow platform builders to add their own layers of functionality on top of that.

The flexibility of allowing any key to sign for any entity means that platform builders can also design their own hierarchies, which includes automatic support for single entities to contain multiple signing keys--the public versions of which can be contained in the signing entity's JWT. Signing authorities being able to have multiple keys means that you can do things like separate the self-signing key from the issuance key, a common practice to prevent the "minting" of new entities by compromised keys.

Much of this work is possible in the x.509 realm, but to do so requires far more work and burdens platform developers with more friction than we're comfortable with.

## Links

Links, References, and Footnotes

[^1]: [What is an x.509 Certificate](https://www.ssl.com/faqs/what-is-an-x-509-certificate/)

[^2]: [PEM, DER, CRT - Encodings and Conventions](https://www.ssl.com/guide/pem-der-crt-and-cer-x-509-encodings-and-conversions/)

[^3]: [Introduction to JSON Web Tokens](https://jwt.io/introduction/)

[^4]: [Claims-Based Identity](https://en.wikipedia.org/wiki/Claims-based_identity)
