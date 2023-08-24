use std::sync::RwLock;

use once_cell::sync::Lazy;

pub(crate) const WASI_BLOBSTORE_BLOBSTORE_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASI_KEYVALUE_ATOMIC_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
    Lazy::new(RwLock::default);
pub(crate) const WASI_KEYVALUE_READWRITE_TARGET: Lazy<RwLock<Option<TargetEntity>>> =
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

mod private {
    pub trait Sealed {}
}

pub trait TargetInterface: private::Sealed {
    fn set_target(&self, target: Option<&TargetEntity>);
}

#[derive(Eq, PartialEq)]
struct WasiBlobstoreBlobstore;
impl private::Sealed for WasiBlobstoreBlobstore {}
impl TargetInterface for WasiBlobstoreBlobstore {
    fn set_target(&self, target: Option<&TargetEntity>) {
        *WASI_BLOBSTORE_BLOBSTORE_TARGET
            .write()
            .expect("failed to lock target") = target.cloned();
    }
}

#[derive(Eq, PartialEq)]
struct WasiKeyvalueAtomic;
impl private::Sealed for WasiKeyvalueAtomic {}
impl TargetInterface for WasiKeyvalueAtomic {
    fn set_target(&self, target: Option<&TargetEntity>) {
        *WASI_KEYVALUE_ATOMIC_TARGET
            .write()
            .expect("failed to lock target") = target.cloned();
    }
}

#[derive(Eq, PartialEq)]
struct WasiKeyvalueReadwrite;
impl private::Sealed for WasiKeyvalueReadwrite {}
impl TargetInterface for WasiKeyvalueReadwrite {
    fn set_target(&self, target: Option<&TargetEntity>) {
        *WASI_KEYVALUE_READWRITE_TARGET
            .write()
            .expect("failed to lock target") = target.cloned();
    }
}

#[derive(Eq, PartialEq)]
struct WasiLoggingLogging;
impl private::Sealed for WasiLoggingLogging {}
impl TargetInterface for WasiLoggingLogging {
    fn set_target(&self, target: Option<&TargetEntity>) {
        *WASI_LOGGING_LOGGING_TARGET
            .write()
            .expect("failed to lock target") = target.cloned();
    }
}

#[derive(Eq, PartialEq)]
struct WasmcloudMessagingConsumer;
impl private::Sealed for WasmcloudMessagingConsumer {}
impl TargetInterface for WasmcloudMessagingConsumer {
    fn set_target(&self, target: Option<&TargetEntity>) {
        *WASMCLOUD_MESSAGING_CONSUMER_TARGET
            .write()
            .expect("failed to lock target") = target.cloned();
    }
}

pub fn target_wasi_blobstore_blobstore() -> &'static dyn TargetInterface {
    &WasiBlobstoreBlobstore
}
pub fn target_wasi_keyvalue_atomic() -> &'static dyn TargetInterface {
    &WasiKeyvalueAtomic
}
pub fn target_wasi_keyvalue_readwrite() -> &'static dyn TargetInterface {
    &WasiKeyvalueReadwrite
}
pub fn target_wasi_logging_logging() -> &'static dyn TargetInterface {
    &WasiLoggingLogging
}
pub fn target_wasmcloud_messaging_consumer() -> &'static dyn TargetInterface {
    &WasmcloudMessagingConsumer
}

pub fn set_target(target: Option<&TargetEntity>, interfaces: &[&dyn TargetInterface]) {
    for interface in interfaces {
        interface.set_target(target)
    }
}
