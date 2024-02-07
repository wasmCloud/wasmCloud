use std::sync::RwLock;

use once_cell::sync::Lazy;

pub(crate) const WASI_BLOBSTORE_BLOBSTORE_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASI_KEYVALUE_ATOMIC_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASI_KEYVALUE_EVENTUAL_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASI_LOGGING_LOGGING_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASMCLOUD_MESSAGING_CONSUMER_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);

/// Actor identifer
#[derive(Clone, Eq, PartialEq)]
pub enum ActorIdentifier {
    /// Actor public key
    PublicKey(String),
    /// Actor call alias
    Alias(String),
}

/// Target entity
#[derive(Clone, Eq, PartialEq)]
pub enum TargetEntity {
    /// Link target paired with an optional name
    Link(Option<String>),
    /// Actor target
    Actor(ActorIdentifier),
}

pub enum TargetInterface {
    WasiBlobstoreBlobstore,
    WasiKeyvalueAtomic,
    WasiKeyvalueEventual,
    WasiLoggingLogging,
    WasmcloudMessagingConsumer,
    Custom(String),
}

impl TargetInterface {
    pub fn wasi_blobstore_blobstore() -> TargetInterface {
        TargetInterface::WasiBlobstoreBlobstore
    }
    pub fn wasi_keyvalue_atomic() -> TargetInterface {
        TargetInterface::WasiKeyvalueAtomic
    }
    pub fn wasi_keyvalue_eventual() -> TargetInterface {
        TargetInterface::WasiKeyvalueEventual
    }
    pub fn wasi_logging_logging() -> TargetInterface {
        TargetInterface::WasiLoggingLogging
    }
    pub fn wasmcloud_messaging_consumer() -> TargetInterface {
        TargetInterface::WasmcloudMessagingConsumer
    }
}

pub fn set_target(target: Option<&TargetEntity>, interfaces: Vec<TargetInterface>) {
    for interface in interfaces {
        match interface {
            TargetInterface::WasiBlobstoreBlobstore => {
                *WASI_BLOBSTORE_BLOBSTORE_TARGET
                    .write()
                    .expect("failed to lock target") = target.cloned();
            }
            TargetInterface::WasiKeyvalueAtomic => {
                *WASI_KEYVALUE_ATOMIC_TARGET
                    .write()
                    .expect("failed to lock target") = target.cloned();
            }

            TargetInterface::WasiKeyvalueEventual => {
                *WASI_KEYVALUE_EVENTUAL_TARGET
                    .write()
                    .expect("failed to lock target") = target.cloned();
            }

            TargetInterface::WasiLoggingLogging => {
                *WASI_LOGGING_LOGGING_TARGET
                    .write()
                    .expect("failed to lock target") = target.cloned();
            }

            TargetInterface::WasmcloudMessagingConsumer => {
                *WASMCLOUD_MESSAGING_CONSUMER_TARGET
                    .write()
                    .expect("failed to lock target") = target.cloned();
            }
            TargetInterface::Custom(_interface) => todo!("not supported yet"),
        }
    }
}
