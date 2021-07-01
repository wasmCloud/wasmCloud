#![allow(unused_imports)]
// avoid false-negative warning for &String parameters in generated CapabilityProvider apis.
#![allow(clippy::ptr_arg)]

// this module is auto-generated from the smithy model
//   file: wasmcloud-core.smithy
//   namespace: org.wasmcloud.core

include!(concat!(env!("OUT_DIR"), "/src/core.rs"));
