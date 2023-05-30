use super::host;

use core::iter;

use serde::Serialize;
use uuid::Uuid;

/// Return a cryptographically-secure pseudo-random [`u64`] value.
pub fn get_random_u64() -> u64 {
    let l = u64::from(random32());
    let r = u64::from(random32());
    debug_assert!(l.leading_zeros() >= u32::BITS);
    l.reverse_bits() | r
}

/// Return `len` cryptographically-secure pseudo-random bytes.
pub fn get_random_bytes(n: u64) -> Vec<u8> {
    let n = n.try_into().expect("too many bytes requested");
    iter::repeat_with(random32)
        .map(u32::to_ne_bytes)
        .flatten()
        .take(n)
        .collect()
}

/// Return a 128-bit value that may contain a pseudo-random value.
///
/// This function is intended to only be called once, by a source language to initialize Denial Of Service (DoS) protection in its hash-map implementation.
pub fn insecure_random() -> (u64, u64) {
    (get_random_u64(), get_random_u64())
}

pub(crate) fn generate_guid() -> Uuid {
    let res = host::call(
        "",
        "wasmcloud:builtin:numbergen",
        "NumberGen.GenerateGuid",
        None,
    )
    .expect("failed to call `NumberGen.GenerateGuid`");
    let id: String =
        rmp_serde::from_slice(&res).expect("failed to decode `NumberGen.GenerateGuid` response");
    Uuid::try_parse(&id).expect("failed to parse UUID")
}

pub(crate) fn random_in_range(min: u32, max: u32) -> u32 {
    #[derive(Serialize)]
    pub struct RangeLimit {
        pub min: u32,
        pub max: u32,
    }
    let pld = rmp_serde::to_vec(&RangeLimit { min, max })
        .expect("failed to serialize `NumberGen.RandomInRange` request");
    let res = host::call(
        "",
        "wasmcloud:builtin:numbergen",
        "NumberGen.RandomInRange",
        Some(&pld),
    )
    .expect("failed to call `NumberGen.RandomInRange`");
    rmp_serde::from_slice(&res).expect("failed to decode `NumberGen.RandomInRange` response")
}

pub(crate) fn random32() -> u32 {
    let res = host::call(
        "",
        "wasmcloud:builtin:numbergen",
        "NumberGen.Random32",
        None,
    )
    .expect("failed to call `NumberGen.Random32`");
    rmp_serde::from_slice(&res).expect("failed to decode `NumberGen.Random32` response")
}
