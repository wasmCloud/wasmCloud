// A default implementation of the "wasmcloud:extras" provider that is always included
// with the host runtime. This provides functionality for generating random numbers,
// generating a guid, and generating a sequence number... things that a standalone
// WASM module cannot do.

use crate::generated::core::HealthResponse;
use crate::generated::extras::{GeneratorRequest, GeneratorResult};
use crate::messagebus::handlers::OP_HEALTH_REQUEST;
extern crate wasmcloud_provider_core as codec;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::OP_BIND_ACTOR;
use codec::{deserialize, serialize};
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;
use wascap::jwt::Claims;

pub(crate) const OP_REQUEST_GUID: &str = "RequestGuid";
pub(crate) const OP_REQUEST_RANDOM: &str = "RequestRandom";
pub(crate) const OP_REQUEST_SEQUENCE: &str = "RequestSequence";

#[derive(Clone)]
pub(crate) struct ExtrasCapabilityProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    sequences: Arc<RwLock<HashMap<String, AtomicU64>>>,
}

impl Default for ExtrasCapabilityProvider {
    fn default() -> Self {
        ExtrasCapabilityProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            sequences: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl ExtrasCapabilityProvider {
    fn generate_guid(
        &self,
        _actor: &str,
        _msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let uuid = Uuid::new_v4();
        let result = GeneratorResult {
            guid: Some(format!("{}", uuid)),
            random_number: 0,
            sequence_number: 0,
        };

        Ok(serialize(&result)?)
    }

    fn generate_random(
        &self,
        _actor: &str,
        msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        use rand::prelude::*;
        let mut rng = rand::thread_rng();
        let result = if let GeneratorRequest {
            random: true,
            min,
            max,
            ..
        } = msg
        {
            let n: u32 = rng.gen_range(min..max);
            GeneratorResult {
                random_number: n,
                sequence_number: 0,
                guid: None,
            }
        } else {
            GeneratorResult::default()
        };

        Ok(serialize(result)?)
    }

    fn generate_sequence(
        &self,
        actor: &str,
        _msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut lock = self.sequences.write().unwrap();
        let seq = lock
            .entry(actor.to_string())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst);
        let result = GeneratorResult {
            sequence_number: seq,
            random_number: 0,
            guid: None,
        };
        Ok(serialize(&result)?)
    }
}

impl CapabilityProvider for ExtrasCapabilityProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn codec::capabilities::Dispatcher>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_REQUEST_GUID => self.generate_guid(actor, deserialize(msg)?),
            OP_REQUEST_RANDOM => self.generate_random(actor, deserialize(msg)?),
            OP_REQUEST_SEQUENCE => self.generate_sequence(actor, deserialize(msg)?),
            OP_HEALTH_REQUEST => healthy(),
            OP_BIND_ACTOR => Ok(vec![]),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {
        // Nothing needed here
    }
}

fn healthy() -> Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>> {
    let hr = HealthResponse {
        message: "".to_string(),
        healthy: true,
    };
    Ok(serialize(hr)?)
}

pub(crate) fn get_claims() -> Claims<wascap::jwt::CapabilityProvider> {
    Claims::<wascap::jwt::CapabilityProvider>::decode(EXTRAS_JWT).unwrap()
}

pub const EXTRAS_JWT: &str = "eyJ0eXAiOiJqd3QiLCJhbGciOiJFZDI1NTE5In0.eyJqdGkiOiJxaFBJdWJMZnRxNE1RdlkxbkQzODJsIiwiaWF0IjoxNjE0ODc3ODM5LCJpc3MiOiJBQ09KSk42V1VQNE9ERDc1WEVCS0tUQ0NVSkpDWTVaS1E1NlhWS1lLNEJFSldHVkFPT1FIWk1DVyIsInN1YiI6IlZESFBLR0ZLREkzNFk0Uk40UFdXWkhSWVo2MzczSFlSU05ORU00VVRETExPR081QjM3VFNWUkVQIiwid2FzY2FwIjp7Im5hbWUiOiJ3YXNtY2xvdWQgRXh0cmFzIiwiY2FwaWQiOiJ3YXNtY2xvdWQ6ZXh0cmFzIiwidmVuZG9yIjoid2FzbWNsb3VkIiwidGFyZ2V0X2hhc2hlcyI6e319fQ.tD3rA1M7fpzyt5-bX1nevdKG4e8GNNjRfNP5Mdz0MHnoZ2R-Qs6APwp2m6PvM9T5-Q4_wBzb_ZpVUoTSi_2RAQ";
pub const EXTRAS_PUBLIC_KEY: &str = "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP";
