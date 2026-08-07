//! Real ports the host binds on a guest's behalf.
//!
//! A guest never holds a listening socket on a real interface. It binds a port
//! inside its virtual loopback — the same bind it does today — and the host
//! binds the real port and splices each accepted connection into that virtual
//! endpoint. The host therefore owns the port table, which is what makes a
//! collision a startup failure rather than a race between two guests, and what
//! makes revoking exposure a matter of dropping a [`PublishedPort`].
//!
//! The publisher does not know or care whether the endpoint on the other side
//! belongs to a host component plugin or to a workload's service: it takes a
//! virtual network and an address in it. [`PortOwner`] exists only to label
//! logs, metrics, and the conflict error.
//!
//! Two properties are load-bearing and easy to lose:
//!
//! - **The guest sees the real peer.** The splice builds its connection with
//!   [`loopback::TcpConn::pair`], whose accepted half carries the address the
//!   external client actually connected from. A plugin doing IP allowlisting or
//!   request logging gets the truth, not a synthetic loopback address.
//! - **Backpressure is preserved.** The virtual transport moves
//!   `(Bytes, OwnedSemaphorePermit)` over *unbounded* channels; the permit is
//!   the only thing bounding them. A splice that ignored it would let an
//!   external peer buffer without limit into host memory on behalf of a guest
//!   that is not reading. See [`splice`].

use core::net::{IpAddr, SocketAddr};
use core::num::NonZeroU16;
use core::time::Duration;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use bytes::Bytes;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tracing::{Instrument as _, debug, info, instrument, trace, warn};

use crate::host::declared_port::Protocol;
use crate::host::quota::{ConnectionSlot, GuestConnectionQuota};
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

/// Host-wide publishing settings.
#[derive(Debug, Clone)]
pub struct PublishConfig {
    /// Whether publishing is permitted at all. Off unless the operator says
    /// otherwise; a declared port on a host with this off is inert and logged.
    pub enabled: bool,
    /// Address real listeners bind. Deliberately not the unspecified address:
    /// an operator opting into publishing still says which interface.
    pub bind_address: IpAddr,
    /// Range a `publish` port must fall in, when set.
    pub port_range: Option<(u16, u16)>,
    /// How many connections one published port serves at once.
    ///
    /// A per-port ceiling, distinct from the owner's `inbound` quota surface:
    /// this stops one exposed port monopolising the owner's inbound
    /// allowance, while the quota stops the owner as a whole monopolising the
    /// host.
    pub max_connections_per_port: usize,
    /// How long an accepted connection waits for the guest's virtual listener
    /// to appear before being reset. Covers both cold start and the gap while a
    /// supervised guest restarts.
    pub readiness_timeout: Duration,
}

impl Default for PublishConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: IpAddr::V4(core::net::Ipv4Addr::LOCALHOST),
            port_range: None,
            max_connections_per_port: 256,
            readiness_timeout: Duration::from_secs(5),
        }
    }
}

impl PublishConfig {
    fn check_in_range(&self, port: u16) -> Result<()> {
        if let Some((lo, hi)) = self.port_range
            && !(lo..=hi).contains(&port)
        {
            bail!("published port {port} is outside the host's --publish-port-range {lo}-{hi}");
        }
        Ok(())
    }
}

/// What an owner needs in order to publish its declared ports: the host's one
/// port table plus that host's publishing settings.
///
/// Bundled because the two are only ever meaningful together — a table without
/// settings cannot publish, and settings without the shared table would let two
/// owners each believe they hold the same address.
#[derive(Debug, Clone)]
pub struct PublishContext {
    pub table: Arc<PortTable>,
    pub config: Arc<PublishConfig>,
    /// The host-level half of the owner's socket policy: enforcement mode,
    /// address ranges, whether host-loopback access is open. `None` leaves the
    /// owner on the permissive default.
    pub socket_policy: Option<Arc<crate::sockets::policy::SocketPolicy>>,
}

impl PublishContext {
    /// A context sharing `table`, with publishing enabled under `config`.
    pub fn new(table: Arc<PortTable>, config: PublishConfig) -> Self {
        Self {
            table,
            config: Arc::new(config),
            socket_policy: None,
        }
    }

    /// Attach the host-level socket policy every guest under this context
    /// inherits.
    #[must_use]
    pub fn with_socket_policy(mut self, policy: Arc<crate::sockets::policy::SocketPolicy>) -> Self {
        self.socket_policy = Some(policy);
        self
    }
}

/// One port to publish: everything about it that is not host-wide.
///
/// Bundled rather than passed as eight positional arguments, most of them
/// addresses and `Arc`s that would substitute for one another silently.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub protocol: Protocol,
    pub owner: PortOwner,
    pub name: Arc<str>,
    /// Port on the host's bind address. Zero takes an ephemeral one.
    pub host_port: u16,
    /// The virtual endpoint inside the owner's network to splice into.
    pub target: SocketAddr,
    pub network: NetworkHandle,
    /// The owner's connection allowance.
    ///
    /// Each served connection — or, for UDP, each peer flow — holds an
    /// `inbound` slot for its lifetime, so an owner's exposed ports draw on the
    /// same ceiling as its outbound sockets and its pooled HTTP rather than
    /// being bounded only per port. `None` leaves it bounded only per port,
    /// which is what a host with no quota registry configures.
    pub quota: Option<GuestConnectionQuota>,
}

impl PublishRequest {
    pub fn new(
        protocol: Protocol,
        owner: PortOwner,
        name: impl Into<Arc<str>>,
        host_port: u16,
        target: SocketAddr,
        network: NetworkHandle,
    ) -> Self {
        Self {
            protocol,
            owner,
            name: name.into(),
            host_port,
            target,
            network,
            quota: None,
        }
    }

    /// Draw this port's connections from `quota`.
    #[must_use]
    pub fn with_quota(mut self, quota: Option<GuestConnectionQuota>) -> Self {
        self.quota = quota;
        self
    }
}

/// How much the splice will buffer per connection, per direction.
///
/// The virtual transport takes one semaphore permit per chunk regardless of
/// chunk size, so bounding memory means bounding both: at most
/// `MAX_INFLIGHT_CHUNKS` chunks of at most `CHUNK_SIZE` bytes are outstanding
/// toward the guest before the splice stops reading from the real socket and
/// lets TCP push back on the external peer.
const CHUNK_SIZE: usize = 64 * 1024;
const MAX_INFLIGHT_CHUNKS: usize = 16;

/// How often the readiness window re-checks for the guest's virtual listener.
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A real listener, bound and serving, spliced into a virtual endpoint.
///
/// Dropping this revokes the exposure: the port-table reservation is released
/// synchronously and the accept loop is aborted, so no further connection is
/// served. The listening socket itself closes once the aborted task is reaped,
/// which is a scheduler pass away rather than immediate — `Drop` cannot await.
/// The gap is observable only as a same-port re-publish failing its OS bind
/// with `AddressInUse` after passing the table check.
///
/// Its lifetime is deliberately the *owner's* lifetime, not one guest
/// incarnation's: a supervised plugin that restarts re-registers its virtual
/// listener while this stays bound, so external clients see a readiness wait
/// rather than a port that vanishes and returns.
#[derive(Debug)]
pub struct PublishedPort {
    owner: PortOwner,
    name: Arc<str>,
    protocol: Protocol,
    local_addr: SocketAddr,
    target: SocketAddr,
    reservation: PortReservation,
    task: tokio::task::JoinHandle<()>,
}

impl PublishedPort {
    /// The real address external clients connect to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The virtual address inside the owner's loopback that this splices into.
    pub fn target(&self) -> SocketAddr {
        self.target
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn owner(&self) -> &PortOwner {
        &self.owner
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }
}

impl Drop for PublishedPort {
    fn drop(&mut self) {
        self.task.abort();
        debug!(
            owner = %self.owner,
            port = %self.name,
            addr = %self.local_addr,
            "unpublished port"
        );
        // `reservation` releases the port table entry on its own drop, after
        // the listener task is gone.
        let _ = &self.reservation;
    }
}

/// Bind `host_port` and splice every accepted connection into `target` inside
/// `network`.
///
/// The listener is bound before this returns, so a port conflict is an error
/// here — at owner start, with a name attached — rather than a mystery later.
/// The guest's virtual listener does *not* need to exist yet: connections
/// arriving before it does wait out
/// [`PublishConfig::readiness_timeout`].
///
/// # Errors
///
/// Fails if publishing is disabled, the port is outside the configured range,
/// the port is already reserved by another owner, or the OS refuses the bind.
#[instrument(skip_all, fields(owner = %request.owner, port = %request.name))]
async fn publish_tcp(
    config: &PublishConfig,
    table: &Arc<PortTable>,
    request: PublishRequest,
) -> Result<PublishedPort> {
    let PublishRequest {
        owner,
        name,
        host_port,
        target,
        network,
        quota,
        ..
    } = request;
    if !config.enabled {
        bail!(
            "{owner} declares published port '{name}', but this host does not allow publishing. \
             Start it with --publish-ports to honor the declaration"
        );
    }
    config.check_in_range(host_port)?;

    let bind_addr = SocketAddr::new(config.bind_address, host_port);
    // Reserve a concrete port before binding, so a clash with another owner is
    // reported as a conflict naming the holder rather than as a bare
    // `AddressInUse` from the OS. An ephemeral request (port 0) has no address
    // to reserve yet.
    let requested = (host_port != 0)
        .then(|| table.reserve(Protocol::Tcp, bind_addr, owner.clone()))
        .transpose()?;

    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind published port {bind_addr} for {owner}"))?;
    let local_addr = listener
        .local_addr()
        .context("failed to read published listener address")?;

    // Key the reservation on what was actually bound. For an ephemeral request
    // the requested address was `:0`, which would make the table answer "no" for
    // the port that is really open — and the table is what tells the rest of the
    // host which addresses it published.
    let reservation = match requested {
        Some(reservation) if reservation.addr() == local_addr => reservation,
        stale => {
            drop(stale);
            table.reserve(Protocol::Tcp, local_addr, owner.clone())?
        }
    };

    let permits = Arc::new(Semaphore::new(config.max_connections_per_port.max(1)));
    let readiness_timeout = config.readiness_timeout;

    let task = tokio::spawn(
        accept_loop(
            listener,
            target,
            network,
            permits,
            quota,
            readiness_timeout,
            owner.clone(),
            Arc::clone(&name),
        )
        .in_current_span(),
    );

    info!(
        addr = %local_addr,
        %target,
        "published port"
    );

    Ok(PublishedPort {
        owner,
        name,
        protocol: Protocol::Tcp,
        local_addr,
        target,
        reservation,
        task,
    })
}

/// Bind and serve a published port of either protocol.
///
/// TCP splices connections, UDP relays datagrams, and both end up as a
/// [`PublishedPort`] whose drop revokes the exposure.
///
/// The one way to publish a port: dispatching on
/// [`PublishRequest::protocol`] here is what keeps a request from reaching a
/// binder for the other protocol.
///
/// # Errors
///
/// Fails if publishing is disabled, the port is outside the configured range,
/// the port is already reserved by another owner, or the OS refuses the bind.
pub async fn publish(
    config: &PublishConfig,
    table: &Arc<PortTable>,
    request: PublishRequest,
) -> Result<PublishedPort> {
    match request.protocol {
        Protocol::Tcp => publish_tcp(config, table, request).await,
        Protocol::Udp => publish_udp(config, table, request).await,
    }
}

/// How long a UDP flow with no traffic is kept before its virtual endpoint is
/// released. UDP has no close, so the only way a mapping ends is by going
/// quiet.
const UDP_FLOW_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Bind `host_port` for UDP and relay datagrams to `target` inside `network`.
///
/// UDP has no connection to splice, so this is a NAT: each external peer is
/// given its own virtual endpoint, and the guest sees datagrams arriving
/// *from* that endpoint. Replying to the address it saw is what routes a
/// datagram back out to that peer — which is the ordinary thing a UDP server
/// does, so the guest needs no special handling.
///
/// # Errors
///
/// Fails if publishing is disabled, the port is outside the configured range,
/// the port is already reserved, or the OS refuses the bind.
#[instrument(skip_all, fields(owner = %request.owner, port = %request.name))]
async fn publish_udp(
    config: &PublishConfig,
    table: &Arc<PortTable>,
    request: PublishRequest,
) -> Result<PublishedPort> {
    let PublishRequest {
        owner,
        name,
        host_port,
        target,
        network,
        quota,
        ..
    } = request;
    if !config.enabled {
        bail!(
            "{owner} declares published port '{name}', but this host does not allow publishing. \
             Start it with --publish-ports to honor the declaration"
        );
    }
    config.check_in_range(host_port)?;

    let bind_addr = SocketAddr::new(config.bind_address, host_port);
    let requested = (host_port != 0)
        .then(|| table.reserve(Protocol::Udp, bind_addr, owner.clone()))
        .transpose()?;

    let socket = Arc::new(
        tokio::net::UdpSocket::bind(bind_addr)
            .await
            .with_context(|| {
                format!("failed to bind published UDP port {bind_addr} for {owner}")
            })?,
    );
    let local_addr = socket
        .local_addr()
        .context("failed to read published UDP listener address")?;

    let reservation = match requested {
        Some(reservation) if reservation.addr() == local_addr => reservation,
        stale => {
            drop(stale);
            table.reserve(Protocol::Udp, local_addr, owner.clone())?
        }
    };

    let max_flows = config.max_connections_per_port.max(1);
    let task = tokio::spawn(
        relay_loop(
            socket,
            target,
            network,
            max_flows,
            quota,
            owner.clone(),
            Arc::clone(&name),
        )
        .in_current_span(),
    );

    info!(addr = %local_addr, %target, "published UDP port");

    Ok(PublishedPort {
        owner,
        name,
        protocol: Protocol::Udp,
        local_addr,
        target,
        reservation,
        task,
    })
}

/// How many bytes one published UDP port may hold queued toward a guest that is
/// not reading. Beyond this, datagrams are dropped — which is what UDP does
/// under load anyway, and is the alternative to buffering into host memory.
///
/// Per published port rather than per host: one busy port must not be able to
/// starve every other port's relay.
const UDP_RELAY_QUEUE_BYTES: usize = 1024 * 1024;

/// One virtual endpoint standing in for one external peer.
struct UdpFlow {
    /// The address the guest sees datagrams arrive from, and replies to.
    virtual_addr: SocketAddr,
    /// The network `virtual_addr` was registered in.
    ///
    /// Held rather than re-read from the [`NetworkHandle`] so that releasing the
    /// port touches the incarnation that allocated it, never a later one that
    /// may have since bound the same ephemeral number.
    net: Arc<Mutex<loopback::Network>>,
    last_seen: tokio::time::Instant,
    /// Drains the guest's replies back out to the peer.
    pump: tokio::task::JoinHandle<()>,
    /// The owner's inbound slot, held for the life of the flow.
    _inbound: Option<ConnectionSlot>,
}

impl Drop for UdpFlow {
    fn drop(&mut self) {
        self.pump.abort();
        // Aborting the pump stops draining replies but leaves the endpoint
        // registered, so releasing the virtual port has to happen here: without
        // it every expired flow keeps an ephemeral port for the life of the
        // incarnation, and a long-lived port eventually exhausts them.
        let Ok(mut net) = self.net.lock() else {
            return;
        };
        if let Some(port) = NonZeroU16::new(self.virtual_addr.port()) {
            net.get_udp_net_mut(self.virtual_addr.ip()).remove(&port);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn relay_loop(
    socket: Arc<tokio::net::UdpSocket>,
    target: SocketAddr,
    network: NetworkHandle,
    max_flows: usize,
    quota: Option<GuestConnectionQuota>,
    owner: PortOwner,
    name: Arc<str>,
) {
    let mut flows: BTreeMap<SocketAddr, UdpFlow> = BTreeMap::new();
    let mut buf = vec![0u8; crate::sockets::MAX_UDP_DATAGRAM_SIZE];
    let mut sweep = tokio::time::interval(UDP_FLOW_IDLE_TIMEOUT / 2);
    let queue = Arc::new(Semaphore::new(UDP_RELAY_QUEUE_BYTES));

    loop {
        tokio::select! {
            _ = sweep.tick() => {
                let now = tokio::time::Instant::now();
                flows.retain(|peer, flow| {
                    let live = now.duration_since(flow.last_seen) < UDP_FLOW_IDLE_TIMEOUT;
                    if !live {
                        trace!(%owner, port = %name, %peer, "released idle UDP flow");
                    }
                    live
                });
            }
            received = socket.recv_from(&mut buf) => {
                let (len, peer) = match received {
                    Ok(received) => received,
                    Err(err) => {
                        warn!(%err, "published UDP port failed to receive; continuing");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                };
                let Some(data) = buf.get(..len).map(<[u8]>::to_vec) else {
                    continue;
                };

                if !flows.contains_key(&peer) {
                    if flows.len() >= max_flows {
                        debug!(%owner, port = %name, %peer, "published UDP port at its flow cap; dropping");
                        continue;
                    }
                    // A flow is this protocol's connection: it holds a virtual
                    // endpoint and a pump task until it goes idle, so it draws
                    // on the owner's inbound allowance the way an accepted TCP
                    // connection does.
                    let inbound = match quota.as_ref() {
                        Some(quota) => match quota.try_acquire_inbound() {
                            Some(slot) => Some(slot),
                            None => {
                                debug!(%owner, port = %name, %peer, "owner is at its inbound connection quota; dropping");
                                continue;
                            }
                        },
                        None => None,
                    };
                    match open_flow(&network, &socket, peer, target, inbound) {
                        Ok(flow) => {
                            flows.insert(peer, flow);
                        }
                        Err(err) => {
                            debug!(%owner, port = %name, %peer, %err, "could not open UDP flow");
                            continue;
                        }
                    }
                }
                let Some(flow) = flows.get_mut(&peer) else {
                    continue;
                };
                flow.last_seen = tokio::time::Instant::now();
                if let Err(err) = send_to_guest(&network, &queue, flow.virtual_addr, target, data) {
                    debug!(%owner, port = %name, %peer, %err, "dropping datagram");
                    // The guest's endpoint went away; forget the flow so the
                    // next datagram from this peer rebuilds it.
                    flows.remove(&peer);
                }
            }
        }
    }
}

/// Give `peer` its own virtual endpoint and start draining the guest's replies
/// back out to it.
fn open_flow(
    network: &NetworkHandle,
    socket: &Arc<tokio::net::UdpSocket>,
    peer: SocketAddr,
    target: SocketAddr,
    inbound: Option<ConnectionSlot>,
) -> Result<UdpFlow> {
    let Some(net) = network.current() else {
        bail!("no incarnation is running to relay to");
    };
    // Port 0 takes an ephemeral virtual port, which is what makes each peer
    // distinguishable to the guest.
    let ephemeral = SocketAddr::new(target.ip(), 0);
    let (virtual_addr, mut rx) = {
        let mut net = net
            .lock()
            .map_err(|e| anyhow::anyhow!("loopback network lock poisoned: {e}"))?;
        net.bind_udp(ephemeral)
            .map_err(|e| anyhow::anyhow!("failed to bind a virtual UDP endpoint: {e:?}"))?
    };

    let socket = Arc::clone(socket);
    let pump = tokio::spawn(async move {
        while let Some((datagram, permit)) = rx.recv().await {
            // Release the guest's capacity once the bytes are ours.
            drop(permit);
            if socket.send_to(&datagram.data, peer).await.is_err() {
                break;
            }
        }
    });

    Ok(UdpFlow {
        virtual_addr,
        net,
        last_seen: tokio::time::Instant::now(),
        pump,
        _inbound: inbound,
    })
}

/// Deliver one datagram to the guest, as though it came from `from`.
///
/// `queue` bounds, in bytes, what this published port may hold outstanding
/// toward a guest that is not reading; each permit is returned when the guest
/// consumes the datagram.
fn send_to_guest(
    network: &NetworkHandle,
    queue: &Arc<Semaphore>,
    from: SocketAddr,
    target: SocketAddr,
    data: Vec<u8>,
) -> Result<()> {
    let Some(net) = network.current() else {
        bail!("no incarnation is running");
    };
    let mut net = net
        .lock()
        .map_err(|e| anyhow::anyhow!("loopback network lock poisoned: {e}"))?;
    let Some(tx) = net
        .connect_udp(&from, &target)
        .map_err(|e| anyhow::anyhow!("virtual UDP lookup failed: {e:?}"))?
    else {
        bail!("nothing is bound on {target} in the guest's virtual network");
    };
    let permit = Arc::clone(queue)
        .try_acquire_many_owned(u32::try_from(data.len().max(1)).unwrap_or(u32::MAX))
        .map_err(|_| anyhow::anyhow!("relay is over its queue ceiling"))?;
    tx.send((
        loopback::UdpDatagram {
            source_address: from,
            data,
        },
        permit,
    ))
    .map_err(|_| anyhow::anyhow!("guest's virtual endpoint closed"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn accept_loop(
    listener: TcpListener,
    target: SocketAddr,
    network: NetworkHandle,
    permits: Arc<Semaphore>,
    quota: Option<GuestConnectionQuota>,
    readiness_timeout: Duration,
    owner: PortOwner,
    name: Arc<str>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                // A per-connection accept error (EMFILE, a peer that reset
                // between SYN and accept) must not take the listener down.
                warn!(%err, "published port failed to accept; continuing");
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };

        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            debug!(
                %peer,
                "published port at its connection cap; resetting"
            );
            drop(stream);
            continue;
        };

        // The per-port permit bounds this one port; the quota bounds the owner
        // across every port it exposes and every connection it makes, and rolls
        // up into the host-wide ceiling. Refused rather than awaited, so a
        // saturated owner resets promptly instead of holding the peer open.
        let inbound = match quota.as_ref() {
            Some(quota) => match quota.try_acquire_inbound() {
                Some(slot) => Some(slot),
                None => {
                    debug!(
                        %owner,
                        port = %name,
                        %peer,
                        "owner is at its inbound connection quota; resetting"
                    );
                    drop(stream);
                    continue;
                }
            },
            None => None,
        };

        let network = network.clone();
        let owner = owner.clone();
        let name = Arc::clone(&name);
        tokio::spawn(async move {
            let _permit = permit;
            let _inbound = inbound;
            if let Err(err) = serve_one(stream, peer, target, network, readiness_timeout).await {
                debug!(%owner, port = %name, %peer, %err, "published connection ended");
            }
        });
    }
}

/// Hand one accepted connection to the guest and pump it until either side
/// closes.
async fn serve_one(
    stream: TcpStream,
    peer: SocketAddr,
    target: SocketAddr,
    network: NetworkHandle,
    readiness_timeout: Duration,
) -> Result<()> {
    // Nagle interacts badly with the request/response traffic that dominates
    // here, and the guest cannot set this itself — it never sees the real
    // socket.
    if let Err(err) = stream.set_nodelay(true) {
        trace!(%err, "failed to set TCP_NODELAY on published connection");
    }

    let accept_tx = wait_for_listener(&network, target, readiness_timeout).await?;

    // `pair(peer, target)` puts `target` on the accepted half's local address —
    // which `loopback::TcpSocket::accept` requires to match the listener — and
    // `peer` on its remote address, so the guest sees the real client.
    let (host_side, guest_side) = loopback::TcpConn::pair(peer, target);
    accept_tx
        .send(guest_side)
        .await
        .map_err(|_| anyhow::anyhow!("guest stopped listening on {target} before accept"))?;

    splice(stream, host_side).await
}

/// Wait for the guest to register a listener at `target`, up to `timeout`.
///
/// A guest's virtual listener is registered from inside its run loop, so there
/// is a real interval after the real port is bound during which nothing is
/// listening behind it. The same interval reopens whenever a supervised guest
/// restarts. Holding the connection through it is the difference between "first
/// request after deploy fails" and "first request after deploy is a little
/// slow".
async fn wait_for_listener(
    network: &NetworkHandle,
    target: SocketAddr,
    timeout: Duration,
) -> Result<mpsc::Sender<loopback::TcpConn>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut waited = false;
    loop {
        // Re-read the handle every pass, not once: if the owner restarted while
        // this connection was waiting, the listener it is waiting for lives in
        // the *new* incarnation's network.
        //
        // Scoped so the lock is not held across the sleep.
        let found = {
            match network.current() {
                Some(net) => {
                    let mut net = net
                        .lock()
                        .map_err(|e| anyhow::anyhow!("loopback network lock poisoned: {e}"))?;
                    net.connect_tcp(&target).ok().cloned()
                }
                // No incarnation is running: keep waiting, it may be restarting.
                None => None,
            }
        };
        if let Some(tx) = found {
            if waited {
                trace!(%target, "guest listener appeared during the readiness window");
            }
            return Ok(tx);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "no guest listener appeared on {target} within the readiness window ({timeout:?})"
            );
        }
        waited = true;
        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }
}

/// Pump bytes between the real socket and the virtual connection until either
/// direction ends.
///
/// Both directions are bounded, and it matters that they are bounded
/// differently:
///
/// - **Toward the guest**, this side owns the semaphore. It reads at most
///   [`CHUNK_SIZE`] at a time and will not have more than
///   [`MAX_INFLIGHT_CHUNKS`] permits outstanding, so a guest that stops reading
///   stops this loop from reading, which lets TCP push back on the external
///   peer instead of growing host memory.
/// - **From the guest**, the guest owns the semaphore. Each chunk arrives
///   carrying a permit that is released only when this side drops it, which
///   happens after the write to the real socket completes. A slow external peer
///   therefore blocks the guest's writes rather than queueing without limit.
async fn splice(stream: TcpStream, conn: loopback::TcpConn) -> Result<()> {
    let loopback::TcpConn { rx, tx, .. } = conn;
    let mut from_guest = rx.context("virtual connection was created without a receiver")?;
    let to_guest = tx.context("virtual connection was created without a sender")?;

    let (mut read_half, mut write_half) = stream.into_split();
    let permits = Arc::new(Semaphore::new(MAX_INFLIGHT_CHUNKS));

    let inbound = async move {
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            // Take the permit *before* reading, so a guest that is not
            // draining stops this task inside `acquire` rather than after it
            // has already pulled bytes off the socket.
            let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
                return Ok::<(), anyhow::Error>(());
            };
            let n = read_half.read(&mut buf).await?;
            let Some(read) = buf.get(..n) else {
                // `read` never reports more than the buffer length.
                return Ok(());
            };
            if read.is_empty() {
                return Ok(());
            }
            let chunk = Bytes::copy_from_slice(read);
            if to_guest.send((chunk, permit)).is_err() {
                // Guest dropped its receive half.
                return Ok(());
            }
        }
    };

    let outbound = async move {
        while let Some((chunk, permit)) = from_guest.recv().await {
            let result = write_half.write_all(&chunk).await;
            // Release the guest's permit only once the bytes are actually
            // gone, so its next send waits on the external peer.
            drop::<OwnedSemaphorePermit>(permit);
            result?;
        }
        // Guest closed its side: half-close ours so the peer sees EOF.
        let _ = write_half.shutdown().await;
        Ok::<(), anyhow::Error>(())
    };

    tokio::select! {
        result = inbound => result,
        result = outbound => result,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn test_config() -> PublishConfig {
        PublishConfig {
            enabled: true,
            bind_address: IpAddr::V4(core::net::Ipv4Addr::LOCALHOST),
            readiness_timeout: Duration::from_millis(300),
            ..Default::default()
        }
    }

    #[test]
    fn a_reservation_conflict_names_the_holder() {
        let table = PortTable::new();
        let _first = table
            .reserve(
                Protocol::Tcp,
                addr("127.0.0.1:31000"),
                PortOwner::Plugin("gateway".into()),
            )
            .unwrap();

        let err = table
            .reserve(
                Protocol::Tcp,
                addr("127.0.0.1:31000"),
                PortOwner::Workload("w-1".into()),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("host plugin 'gateway'"), "got: {err}");
    }

    #[test]
    fn releasing_a_reservation_frees_the_port() {
        let table = PortTable::new();
        let owner = PortOwner::Plugin("gateway".into());
        let reservation = table
            .reserve(Protocol::Tcp, addr("127.0.0.1:31000"), owner.clone())
            .unwrap();
        assert!(table.is_published(Protocol::Tcp, addr("127.0.0.1:31000")));

        drop(reservation);
        assert!(!table.is_published(Protocol::Tcp, addr("127.0.0.1:31000")));
        // And the port can be claimed again.
        table
            .reserve(Protocol::Tcp, addr("127.0.0.1:31000"), owner)
            .unwrap();
    }

    #[test]
    fn tcp_and_udp_reservations_do_not_collide() {
        let table = PortTable::new();
        let owner = PortOwner::Plugin("gateway".into());
        let _tcp = table
            .reserve(Protocol::Tcp, addr("127.0.0.1:31000"), owner.clone())
            .unwrap();
        table
            .reserve(Protocol::Udp, addr("127.0.0.1:31000"), owner)
            .unwrap();
    }

    #[tokio::test]
    async fn publishing_is_refused_unless_the_host_enabled_it() {
        let table = PortTable::new();
        let config = PublishConfig::default();
        let err = publish_tcp(
            &config,
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                31000,
                addr("127.0.0.1:50051"),
                NetworkHandle::new(),
            ),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("--publish-ports"), "got: {err}");
        // A refused publish leaves no reservation behind.
        assert!(!table.is_published(Protocol::Tcp, addr("127.0.0.1:31000")));
    }

    #[tokio::test]
    async fn a_port_outside_the_configured_range_is_refused() {
        let table = PortTable::new();
        let config = PublishConfig {
            port_range: Some((31000, 32767)),
            ..test_config()
        };
        let err = publish_tcp(
            &config,
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                8080,
                addr("127.0.0.1:50051"),
                NetworkHandle::new(),
            ),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("publish-port-range"), "got: {err}");
    }

    /// An ephemeral request must end up recorded under the port it actually got,
    /// not under `:0` — the table is what tells the rest of the host which
    /// addresses it published.
    #[tokio::test]
    async fn an_ephemeral_publish_is_reserved_under_the_port_it_got() {
        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                0,
                addr("127.0.0.1:50051"),
                NetworkHandle::new(),
            ),
        )
        .await
        .unwrap();

        assert_ne!(published.local_addr().port(), 0);
        assert!(table.is_published(Protocol::Tcp, published.local_addr()));
        assert!(!table.is_published(Protocol::Tcp, addr("127.0.0.1:0")));
    }

    /// The real listener must be bound before `publish_tcp` returns, so a
    /// conflict is an owner-start failure rather than a surprise later.
    #[tokio::test]
    async fn the_real_port_is_bound_before_the_guest_exists() {
        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                0,
                addr("127.0.0.1:50051"),
                NetworkHandle::new(),
            ),
        )
        .await
        .unwrap();

        // Nothing is listening in the virtual network, but the real port
        // accepts: the connection waits out the readiness window.
        let connected = TcpStream::connect(published.local_addr()).await;
        assert!(connected.is_ok(), "real port should accept immediately");
    }

    #[tokio::test]
    async fn dropping_a_published_port_closes_the_listener() {
        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                0,
                addr("127.0.0.1:50051"),
                NetworkHandle::new(),
            ),
        )
        .await
        .unwrap();
        let real_addr = published.local_addr();
        assert!(table.is_published(Protocol::Tcp, real_addr));

        drop(published);
        // The reservation goes synchronously.
        assert!(!table.is_published(Protocol::Tcp, real_addr));

        // The socket closes when the aborted accept task is reaped, which is a
        // scheduler pass away rather than immediate.
        let freed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(listener) = TcpListener::bind(real_addr).await {
                    return listener;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            freed.is_ok(),
            "port should be free shortly after the publisher is dropped"
        );
    }

    /// Stand in for a guest registering a virtual listener from its run loop.
    fn register_listener(
        network: &Arc<Mutex<loopback::Network>>,
        target: SocketAddr,
    ) -> mpsc::Receiver<loopback::TcpConn> {
        let mut net = network.lock().unwrap();
        net.bind_tcp(target).unwrap();
        let (tx, rx) = mpsc::channel(4);
        net.get_tcp_net_mut(target.ip()).insert(
            target.port().try_into().unwrap(),
            loopback::TcpEndpoint::Listening(tx),
        );
        rx
    }

    #[tokio::test]
    async fn the_readiness_window_gives_up_with_a_reason() {
        let network = NetworkHandle::new();
        network.replace();
        let err = wait_for_listener(&network, addr("127.0.0.1:50051"), Duration::from_millis(80))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("readiness window"), "got: {err}");
    }

    #[tokio::test]
    async fn the_readiness_window_succeeds_once_the_listener_appears() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:50051");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            let _rx = register_listener(&current, target);
            std::mem::forget(_rx);
        });

        wait_for_listener(&network, target, Duration::from_secs(2))
            .await
            .expect("listener registered inside the window should be found");
    }

    fn quota(inbound: usize) -> GuestConnectionQuota {
        GuestConnectionQuota::new(
            crate::host::quota::QuotaLimits {
                http: 8,
                sockets: 8,
                inbound,
            },
            None,
        )
    }

    /// The per-port cap bounds one port; the owner's inbound quota is what
    /// bounds the owner across every port it exposes, and is the tier that
    /// rolls up into the host-wide ceiling.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_owner_at_its_inbound_quota_stops_serving_connections() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:50055");
        let mut accepts = register_listener(&current, target);

        let quota = quota(1);
        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                0,
                target,
                network,
            )
            .with_quota(Some(quota.clone())),
        )
        .await
        .unwrap();

        // The first connection is served, and holds the owner's only slot.
        let _first = TcpStream::connect(published.local_addr()).await.unwrap();
        let _served = tokio::time::timeout(Duration::from_secs(5), accepts.recv())
            .await
            .expect("the first connection should reach the guest")
            .expect("the listener channel should be open");
        assert_eq!(quota.inbound_available(), 0);

        // The second is accepted by the OS and then closed: the owner has
        // nothing left to spend on it.
        let mut second = TcpStream::connect(published.local_addr()).await.unwrap();
        let mut buf = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(5), second.read(&mut buf))
            .await
            .expect("a refused connection should close promptly, not hang");
        match read {
            // Either shape is a refusal; which one depends on the platform.
            Ok(0) | Err(_) => {}
            Ok(n) => panic!("expected a closed connection, read {n} bytes"),
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), accepts.recv())
                .await
                .is_err(),
            "a connection over the owner's quota must never reach the guest"
        );
    }

    /// A UDP flow is this protocol's connection — it holds a virtual endpoint
    /// and a task until it goes idle — so it draws on the same allowance.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_udp_relay_stops_opening_flows_at_the_inbound_quota() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:53532");
        let mut guest_rx = {
            let mut net = current.lock().unwrap();
            net.bind_udp(target).unwrap().1
        };

        let quota = quota(1);
        let table = PortTable::new();
        let published = publish_udp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Udp,
                PortOwner::Plugin("gateway".into()),
                "dns",
                0,
                target,
                network,
            )
            .with_quota(Some(quota.clone())),
        )
        .await
        .unwrap();

        let first = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        first.send_to(b"one", published.local_addr()).await.unwrap();
        let (datagram, permit) = tokio::time::timeout(Duration::from_secs(5), guest_rx.recv())
            .await
            .expect("the first peer should reach the guest")
            .unwrap();
        assert_eq!(&datagram.data, b"one");
        drop(permit);
        assert_eq!(quota.inbound_available(), 0);

        // A second peer needs a second flow, and there is nothing left to open
        // it with. UDP's answer to that is to drop the datagram.
        let second = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        second
            .send_to(b"two", published.local_addr())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), guest_rx.recv())
                .await
                .is_err(),
            "a peer over the owner's quota must not be relayed"
        );
    }

    /// A flow's endpoint is registered in the guest's network and nothing else
    /// unregisters it, so ending the flow is what has to give the ephemeral
    /// port back — otherwise a long-lived published port accumulates one per
    /// peer it has ever seen.
    #[tokio::test]
    async fn an_ended_udp_flow_gives_back_its_port_and_its_slot() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:53540");
        {
            current.lock().unwrap().bind_udp(target).unwrap();
        }

        let quota = quota(2);
        let slot = quota.try_acquire_inbound();
        assert_eq!(quota.inbound_available(), 1);

        let socket = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let flow = open_flow(&network, &socket, addr("127.0.0.1:9999"), target, slot).unwrap();
        let virtual_addr = flow.virtual_addr;
        let port = NonZeroU16::new(virtual_addr.port()).expect("an ephemeral port is never zero");
        assert!(
            current
                .lock()
                .unwrap()
                .get_udp_net_mut(virtual_addr.ip())
                .contains_key(&port)
        );

        drop(flow);

        assert!(
            !current
                .lock()
                .unwrap()
                .get_udp_net_mut(virtual_addr.ip())
                .contains_key(&port),
            "an ended flow must release its virtual port"
        );
        assert_eq!(
            quota.inbound_available(),
            2,
            "an ended flow must return its inbound slot"
        );
    }

    /// The relay is a NAT, so the round trip is what matters: the guest must
    /// see a source address it can reply to, and replying there must reach the
    /// original peer.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_udp_flow_round_trips_and_gives_the_guest_a_repliable_address() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:53530");

        // Stand in for a guest binding its virtual UDP port.
        let mut guest_rx = {
            let mut net = current.lock().unwrap();
            let (bound, rx) = net.bind_udp(target).unwrap();
            assert_eq!(bound, target);
            rx
        };

        let table = PortTable::new();
        let published = publish_udp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Udp,
                PortOwner::Plugin("gateway".into()),
                "dns",
                0,
                target,
                network.clone(),
            ),
        )
        .await
        .unwrap();

        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer.send_to(b"question", published.local_addr())
            .await
            .unwrap();

        let (datagram, permit) = tokio::time::timeout(Duration::from_secs(5), guest_rx.recv())
            .await
            .expect("relay should deliver to the guest")
            .expect("channel should be open");
        assert_eq!(&datagram.data, b"question");
        drop(permit);

        // The guest replies to the address it saw — an ephemeral virtual port
        // the relay minted for this peer, not the peer's real address.
        let reply_to = datagram.source_address;
        assert_ne!(reply_to, target, "the flow needs its own endpoint");
        {
            let mut net = current.lock().unwrap();
            let tx = net
                .connect_udp(&target, &reply_to)
                .unwrap()
                .expect("the relay's endpoint should be reachable")
                .clone();
            let permits = Arc::new(Semaphore::new(8));
            let permit = permits.try_acquire_owned().unwrap();
            tx.send((
                loopback::UdpDatagram {
                    source_address: target,
                    data: b"answer".to_vec(),
                },
                permit,
            ))
            .unwrap();
        }

        let mut buf = [0u8; 16];
        let (len, from) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut buf))
            .await
            .expect("the reply should reach the original peer")
            .unwrap();
        assert_eq!(&buf[..len], b"answer");
        assert_eq!(from, published.local_addr());
    }

    /// Two peers must not share one virtual endpoint, or the guest cannot tell
    /// them apart and its replies go to whichever it saw last.
    #[tokio::test(flavor = "multi_thread")]
    async fn each_udp_peer_gets_its_own_virtual_endpoint() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:53531");
        let mut guest_rx = {
            let mut net = current.lock().unwrap();
            net.bind_udp(target).unwrap().1
        };

        let table = PortTable::new();
        let published = publish_udp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Udp,
                PortOwner::Plugin("gateway".into()),
                "dns",
                0,
                target,
                network,
            ),
        )
        .await
        .unwrap();

        let mut sources = Vec::new();
        for _ in 0..2 {
            let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            peer.send_to(b"hi", published.local_addr()).await.unwrap();
            let (datagram, permit) = tokio::time::timeout(Duration::from_secs(5), guest_rx.recv())
                .await
                .expect("relay should deliver")
                .unwrap();
            drop(permit);
            sources.push(datagram.source_address);
        }
        assert_ne!(
            sources[0], sources[1],
            "two peers must map to two virtual endpoints"
        );
    }

    /// The whole point of routing through [`NetworkHandle`]: a restart swaps the
    /// network out from under a connection that is already waiting, and the wait
    /// has to follow it rather than poll a dead one until it times out.
    #[tokio::test]
    async fn the_readiness_window_follows_a_restart_to_the_new_network() {
        let network = NetworkHandle::new();
        // The incarnation that was running when the connection arrived, with no
        // listener registered — it faulted before getting that far.
        network.replace();
        let target = addr("127.0.0.1:50051");

        let restarting = network.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // Supervisor installs the next incarnation's network...
            let next = restarting.replace();
            tokio::time::sleep(Duration::from_millis(20)).await;
            // ...which then registers its listener.
            let rx = register_listener(&next, target);
            std::mem::forget(rx);
        });

        wait_for_listener(&network, target, Duration::from_secs(2))
            .await
            .expect("a wait in progress should find the listener of the new incarnation");
    }

    /// Bytes have to actually move, in both directions, and the guest has to see
    /// the real client rather than a synthetic loopback address.
    #[tokio::test]
    async fn a_spliced_connection_round_trips_and_carries_the_real_peer() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:50051");
        let mut accepts = register_listener(&current, target);

        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "grpc",
                0,
                target,
                network,
            ),
        )
        .await
        .unwrap();

        let mut client = TcpStream::connect(published.local_addr()).await.unwrap();
        let client_addr = client.local_addr().unwrap();

        let guest = accepts.recv().await.expect("splice should deliver a conn");
        // `accept()` matches on this, and the guest reads its peer from it.
        assert_eq!(guest.local_address, target);
        assert_eq!(
            guest.remote_address, client_addr,
            "guest should see the real external client, not a synthetic address"
        );

        let mut guest_rx = guest.rx.expect("accepted conn has a receiver");
        let guest_tx = guest.tx.expect("accepted conn has a sender");

        client.write_all(b"ping").await.unwrap();
        let (chunk, permit) = guest_rx.recv().await.expect("guest should receive");
        assert_eq!(&chunk[..], b"ping");
        drop(permit);

        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.acquire_owned().await.unwrap();
        guest_tx
            .send((Bytes::from_static(b"pong"), permit))
            .unwrap();

        let mut buf = [0u8; 4];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
    }

    /// A guest that stops reading must stop the splice from reading, so an
    /// external peer cannot buffer without limit into host memory.
    #[tokio::test]
    async fn a_guest_that_stops_reading_stops_the_splice_from_reading() {
        let network = NetworkHandle::new();
        let current = network.replace();
        let target = addr("127.0.0.1:50052");
        let mut accepts = register_listener(&current, target);

        let table = PortTable::new();
        let published = publish_tcp(
            &test_config(),
            &table,
            PublishRequest::new(
                Protocol::Tcp,
                PortOwner::Plugin("gateway".into()),
                "slow",
                0,
                target,
                network,
            ),
        )
        .await
        .unwrap();

        let mut client = TcpStream::connect(published.local_addr()).await.unwrap();
        let guest = accepts.recv().await.unwrap();
        // Hold the receiver without draining it: every permit stays taken.
        let _guest_rx = guest.rx.expect("accepted conn has a receiver");

        // Write far more than the splice may hold. The socket buffers absorb
        // some, then the write blocks because the splice stopped reading.
        let payload = vec![0u8; CHUNK_SIZE];
        let mut written = 0usize;
        let ceiling = CHUNK_SIZE * MAX_INFLIGHT_CHUNKS * 64;
        loop {
            match tokio::time::timeout(Duration::from_millis(200), client.write_all(&payload)).await
            {
                Ok(Ok(())) => written += payload.len(),
                // Blocked: backpressure reached the external peer, which is the
                // property under test.
                Err(_elapsed) => break,
                Ok(Err(err)) => panic!("unexpected write error: {err}"),
            }
            assert!(
                written < ceiling,
                "splice accepted {written} bytes to a guest that never read; \
                 backpressure is not reaching the peer"
            );
        }
    }
}
