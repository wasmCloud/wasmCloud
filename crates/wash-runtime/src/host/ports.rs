//! The host's record of which real ports are spoken for.
//!
//! The host owns the port table, which is what makes a collision a startup
//! failure rather than a race between two guests. [`PortOwner`] exists only to
//! label logs, metrics, and the conflict error; nothing branches on it.
//!
//! [`SocketPolicy`](crate::sockets::policy::SocketPolicy) reads this to refuse a
//! guest that dials a port the host itself published — its own ingress, or
//! another tenant's service — through the host-loopback sentinel.

use core::net::SocketAddr;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::host::declared_port::Protocol;
use crate::sockets::loopback;

/// Who holds a port reservation. Labels only — the publisher's behavior does
/// not branch on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortOwner {
    Workload(Arc<str>),
    Plugin(Arc<str>),
}

impl core::fmt::Display for PortOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Workload(id) => write!(f, "workload '{id}'"),
            Self::Plugin(id) => write!(f, "host plugin '{id}'"),
        }
    }
}

/// A virtual network that survives its owner being restarted.
///
/// A supervised guest gets a *fresh* [`loopback::Network`] per incarnation, and
/// tearing a store down does not unregister the virtual ports its sockets held
/// — those registrations are released by the guest dropping each socket, which
/// a faulted incarnation never gets to do. Sharing one network across
/// incarnations would therefore leave the old listener's port occupied and the
/// new incarnation's bind failing with `AddressInUse` forever.
///
/// So the network is replaced on each restart and reached through this handle.
/// A [`PublishedPort`] holds the handle rather than the network, which is what
/// lets the real listener stay bound across a restart: connections arriving
/// while the new incarnation is coming up simply wait out the readiness window
/// against the new network.
#[derive(Clone, Default)]
pub struct NetworkHandle(Arc<arc_swap::ArcSwapOption<Mutex<loopback::Network>>>);

impl NetworkHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point at a network someone else owns, for an owner whose network is not
    /// replaced per incarnation.
    ///
    /// A workload's virtual network is shared by its service *and* its
    /// components — that sharing is what lets a component dial its service — so
    /// it cannot be swapped out when the service restarts the way a plugin's
    /// private one can.
    pub fn pinned(network: Arc<Mutex<loopback::Network>>) -> Self {
        Self(Arc::new(arc_swap::ArcSwapOption::from(Some(network))))
    }

    /// Install a fresh network for a new incarnation and return it, for the
    /// store being built to use directly.
    pub fn replace(&self) -> Arc<Mutex<loopback::Network>> {
        let network = Arc::new(Mutex::new(loopback::Network::default()));
        self.0.store(Some(Arc::clone(&network)));
        network
    }

    /// The network of the current incarnation, if one is running.
    pub fn current(&self) -> Option<Arc<Mutex<loopback::Network>>> {
        self.0.load_full()
    }

    /// Drop the current network, e.g. because the owner stopped for good.
    pub fn clear(&self) {
        self.0.store(None);
    }
}

impl core::fmt::Debug for NetworkHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NetworkHandle")
            .field("live", &self.current().is_some())
            .finish()
    }
}

type PortKey = (Protocol, SocketAddr);

/// The host's single record of which real ports are spoken for.
///
/// Every exposure route reserves here — host-published splices, plugin direct
/// binds, and (once it lands) workload published ports — so one lookup answers
/// "is this port taken, and by whom". A reservation is released when its
/// [`PortReservation`] is dropped.
#[derive(Debug, Default)]
pub struct PortTable {
    reserved: Mutex<BTreeMap<PortKey, PortOwner>>,
}

impl PortTable {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Claim `addr` for `owner`.
    ///
    /// # Errors
    ///
    /// Fails if the port is already reserved, naming the current holder so the
    /// operator can find the other declaration.
    pub fn reserve(
        self: &Arc<Self>,
        protocol: Protocol,
        addr: SocketAddr,
        owner: PortOwner,
    ) -> Result<PortReservation> {
        let key = (protocol, addr);
        let mut reserved = self
            .reserved
            .lock()
            .map_err(|e| anyhow::anyhow!("port table lock poisoned: {e}"))?;
        if let Some(holder) = reserved.get(&key) {
            bail!("{protocol} port {addr} is already published by {holder}");
        }
        reserved.insert(key, owner);
        Ok(PortReservation {
            key,
            table: Arc::clone(self),
        })
    }

    /// Whether `addr` is a port this host published. Once the host-loopback
    /// door of the wider design exists, this is what stops a co-tenant reaching
    /// a published service by dialing the machine's own address.
    pub fn is_published(&self, protocol: Protocol, addr: SocketAddr) -> bool {
        self.reserved
            .lock()
            .is_ok_and(|reserved| reserved.contains_key(&(protocol, addr)))
    }

    fn release(&self, key: &PortKey) {
        if let Ok(mut reserved) = self.reserved.lock() {
            reserved.remove(key);
        }
    }
}

/// A live claim on a real port. Releases on drop.
#[derive(Debug)]
pub struct PortReservation {
    key: PortKey,
    table: Arc<PortTable>,
}

impl PortReservation {
    pub fn addr(&self) -> SocketAddr {
        self.key.1
    }
}

impl Drop for PortReservation {
    fn drop(&mut self) {
        self.table.release(&self.key);
    }
}
