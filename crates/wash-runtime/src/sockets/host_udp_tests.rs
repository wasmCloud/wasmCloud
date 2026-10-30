#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::sockets::{AddrDecision, DenyReason, Plane, SocketAddrCheck, WasiSocketsCtx};
use std::time::Duration;
use udp::{
    HostIncomingDatagramStream as Incoming, HostOutgoingDatagramStream as Outgoing,
    HostUdpSocket as Socket,
};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::p2::bindings::sockets::{instance_network, udp_create_socket};

struct Harness {
    ctx: WasiSocketsCtx,
    table: ResourceTable,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            ctx: WasiSocketsCtx {
                socket_addr_check: SocketAddrCheck::new(|addr, _| {
                    AddrDecision::allow_by_address(addr)
                }),
                ..Default::default()
            },
            table: ResourceTable::new(),
        }
    }
}

impl Harness {
    fn view(&mut self) -> WasiSocketsCtxView<'_> {
        WasiSocketsCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }

    async fn create(&mut self, family: IpAddressFamily) -> u32 {
        udp_create_socket::Host::create_udp_socket(&mut self.view(), family)
            .await
            .unwrap()
            .rep()
    }

    async fn bind(&mut self, address: &str) -> (u32, SocketAddr) {
        let addr: SocketAddr = address.parse().unwrap();
        let family = if addr.is_ipv4() {
            IpAddressFamily::Ipv4
        } else {
            IpAddressFamily::Ipv6
        };
        let rep = self.create(family).await;
        let network = instance_network::Host::instance_network(&mut self.view()).unwrap();
        Socket::start_bind(
            &mut self.view(),
            Resource::new_borrow(rep),
            network,
            socket_addr_to_ip_socket_address(addr),
        )
        .await
        .unwrap();
        Socket::finish_bind(&mut self.view(), Resource::new_borrow(rep)).unwrap();
        let addr = Socket::local_address(&mut self.view(), Resource::new_borrow(rep)).unwrap();
        (rep, ip_socket_address_to_socket_addr(addr))
    }

    async fn streams(&mut self, rep: u32, remote: Option<SocketAddr>) -> (u32, u32) {
        let (rx, tx) = Socket::stream(
            &mut self.view(),
            Resource::new_borrow(rep),
            remote.map(socket_addr_to_ip_socket_address),
        )
        .await
        .unwrap();
        (rx.rep(), tx.rep())
    }

    fn check(&mut self, tx: u32) -> u64 {
        Outgoing::check_send(&mut self.view(), Resource::new_borrow(tx)).unwrap()
    }

    fn send(&mut self, tx: u32, data: Vec<u8>, remote: Option<SocketAddr>) -> SocketResult<u64> {
        Outgoing::send(
            &mut self.view(),
            Resource::new_borrow(tx),
            vec![datagram(data, remote)],
        )
    }

    fn receive(&mut self, rx: u32, max: u64) -> Vec<udp::IncomingDatagram> {
        Incoming::receive(&mut self.view(), Resource::new_borrow(rx), max).unwrap()
    }

    async fn ready(&mut self, rx: u32) {
        let stream = self
            .table
            .get_mut(&Resource::<IncomingDatagramStream>::new_borrow(rx))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), stream.ready())
            .await
            .unwrap();
    }

    async fn drop_streams(&mut self, rx: u32, tx: u32) {
        Incoming::drop(&mut self.view(), Resource::new_own(rx)).unwrap();
        Outgoing::drop(&mut self.view(), Resource::new_own(tx))
            .await
            .unwrap();
    }
}

fn datagram(data: Vec<u8>, remote: Option<SocketAddr>) -> udp::OutgoingDatagram {
    udp::OutgoingDatagram {
        data,
        remote_address: remote.map(socket_addr_to_ip_socket_address),
    }
}

fn assert_code<T: std::fmt::Debug>(result: SocketResult<T>, expected: ErrorCode) {
    let error = result.unwrap_err();
    assert_eq!(error.downcast_ref(), Some(&expected), "{error:?}");
}

#[tokio::test]
async fn loopback_roundtrip_preserves_datagrams_sources_and_receive_limits() {
    for local in ["127.0.0.1:0", "[::1]:0", "0.0.0.0:0", "[::]:0"] {
        for connected in [false, true] {
            let mut h = Harness::default();
            let (receiver, peer) = h
                .bind(if local.starts_with('[') {
                    "[::1]:0"
                } else {
                    "127.0.0.1:0"
                })
                .await;
            let (sender, mut source) = h.bind(local).await;
            source.set_ip(peer.ip());
            let (rx, _) = h.streams(receiver, None).await;
            let (_, tx) = h.streams(sender, connected.then_some(peer)).await;
            assert!(h.receive(rx, 10).is_empty());
            for payload in [vec![], vec![0, 255, 1], vec![7; 1024]] {
                assert!(h.check(tx) > 0);
                assert_eq!(
                    h.send(tx, payload.clone(), (!connected).then_some(peer))
                        .unwrap(),
                    1
                );
                h.ready(rx).await;
                assert!(h.receive(rx, 0).is_empty());
                let got = h.receive(rx, 1);
                assert_eq!(got.len(), 1);
                assert_eq!(got[0].data, payload);
                assert_eq!(
                    ip_socket_address_to_socket_addr(got[0].remote_address),
                    source
                );
            }
            for byte in [1, 2, 3] {
                h.check(tx);
                h.send(tx, vec![byte], (!connected).then_some(peer))
                    .unwrap();
            }
            assert_eq!(
                h.receive(rx, 2)
                    .iter()
                    .map(|d| d.data[0])
                    .collect::<Vec<_>>(),
                [1, 2]
            );
            assert_eq!(h.receive(rx, u64::MAX)[0].data, [3]);
        }
    }
}

#[tokio::test]
async fn stream_children_must_be_dropped_before_restreaming_or_dropping_socket() {
    let mut h = Harness::default();
    let socket = h.create(IpAddressFamily::Ipv4).await;
    assert_code(
        Socket::stream(&mut h.view(), Resource::new_borrow(socket), None).await,
        ErrorCode::InvalidState,
    );
    Socket::drop(&mut h.view(), Resource::new_own(socket)).unwrap();
    let (socket, _) = h.bind("127.0.0.1:0").await;
    let (rx, tx) = h
        .streams(socket, Some("127.0.0.1:1234".parse().unwrap()))
        .await;
    assert!(
        Socket::stream(&mut h.view(), Resource::new_borrow(socket), None)
            .await
            .unwrap_err()
            .downcast_ref()
            .is_none()
    );
    assert!(Socket::drop(&mut h.view(), Resource::new_own(socket)).is_err());
    Incoming::drop(&mut h.view(), Resource::new_own(rx)).unwrap();
    assert!(
        Socket::stream(&mut h.view(), Resource::new_borrow(socket), None)
            .await
            .is_err()
    );
    Outgoing::drop(&mut h.view(), Resource::new_own(tx))
        .await
        .unwrap();
    let (rx, tx) = h.streams(socket, None).await;
    assert_code(
        Socket::remote_address(&mut h.view(), Resource::new_borrow(socket)),
        ErrorCode::InvalidState,
    );
    h.drop_streams(rx, tx).await;
    Socket::drop(&mut h.view(), Resource::new_own(socket)).unwrap();
    assert!(Socket::local_address(&mut h.view(), Resource::new_borrow(socket)).is_err());
}

#[tokio::test]
async fn send_enforces_permits_batch_size_and_destination_contract() {
    for local in ["127.0.0.1:0", "0.0.0.0:0"] {
        let mut h = Harness::default();
        let (socket, _) = h.bind(local).await;
        let (rx, tx) = h.streams(socket, None).await;
        let peer = "127.0.0.1:1234".parse().unwrap();
        assert_eq!(
            Outgoing::send(&mut h.view(), Resource::new_borrow(tx), vec![]).unwrap(),
            0
        );
        assert!(
            h.send(tx, vec![1], Some(peer))
                .unwrap_err()
                .downcast_ref()
                .is_none()
        );
        h.check(tx);
        let batch = vec![datagram(vec![1], Some(peer)), datagram(vec![2], Some(peer))];
        assert!(
            Outgoing::send(&mut h.view(), Resource::new_borrow(tx), batch)
                .unwrap_err()
                .downcast_ref()
                .is_none()
        );
        for invalid in [
            None,
            Some("127.0.0.1:0".parse().unwrap()),
            Some("0.0.0.0:9".parse().unwrap()),
            Some("[::1]:9".parse().unwrap()),
        ] {
            h.check(tx);
            assert_code(h.send(tx, vec![1], invalid), ErrorCode::InvalidArgument);
        }
        h.check(tx);
        assert_code(
            h.send(tx, vec![0; MAX_UDP_DATAGRAM_SIZE + 1], Some(peer)),
            ErrorCode::DatagramTooLarge,
        );
        h.check(tx);
        assert_eq!(h.send(tx, vec![1], Some(peer)).unwrap(), 1);
        assert!(
            h.send(tx, vec![1], Some(peer))
                .unwrap_err()
                .downcast_ref()
                .is_none()
        );
        h.drop_streams(rx, tx).await;
        let (_, tx) = h.streams(socket, Some(peer)).await;
        h.check(tx);
        assert_code(
            h.send(tx, vec![1], Some("127.0.0.1:4321".parse().unwrap())),
            ErrorCode::InvalidArgument,
        );
        h.check(tx);
        assert_eq!(h.send(tx, vec![1], Some(peer)).unwrap(), 1);
    }
}

#[tokio::test]
async fn receive_drops_policy_denied_sources() {
    let mut h = Harness::default();
    h.ctx.socket_addr_check = SocketAddrCheck::new(|addr, reason| {
        if matches!(reason, SocketAddrUse::UdpReceive) && addr.port() == 1234 {
            AddrDecision::Deny(DenyReason::NotPermitted)
        } else {
            AddrDecision::allow_by_address(addr)
        }
    });
    let (receiver, peer) = h.bind("127.0.0.1:0").await;
    let (denied, _) = h.bind("127.0.0.1:1234").await;
    let (allowed, source) = h.bind("127.0.0.1:4321").await;
    let (rx, _) = h.streams(receiver, None).await;
    let (_, denied_tx) = h.streams(denied, None).await;
    let (_, allowed_tx) = h.streams(allowed, None).await;
    h.check(denied_tx);
    h.send(denied_tx, vec![1], Some(peer)).unwrap();
    h.check(allowed_tx);
    h.send(allowed_tx, vec![2], Some(peer)).unwrap();
    h.ready(rx).await;
    let got = h.receive(rx, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, [2]);
    assert_eq!(
        ip_socket_address_to_socket_addr(got[0].remote_address),
        source
    );
}

#[tokio::test]
async fn send_buffer_backpressure_recovers_after_receive_and_stream_drop() {
    let mut h = Harness::default();
    let (receiver, peer) = h.bind("127.0.0.1:0").await;
    let (sender, _) = h.bind("127.0.0.1:0").await;
    Socket::set_send_buffer_size(&mut h.view(), Resource::new_borrow(sender), 4).unwrap();
    let (rx, _) = h.streams(receiver, None).await;
    let (sender_rx, tx) = h.streams(sender, None).await;
    assert_eq!(h.check(tx), 1);
    assert_eq!(h.check(tx), 1);
    assert_eq!(h.send(tx, vec![0; 4], Some(peer)).unwrap(), 1);
    assert_eq!(h.check(tx), 0);
    assert_eq!(h.receive(rx, 1).len(), 1);
    assert_eq!(h.check(tx), 1);
    assert_code(
        h.send(tx, vec![0; 5], Some(peer)),
        ErrorCode::DatagramTooLarge,
    );
    assert_eq!(h.check(tx), 1);
    h.drop_streams(sender_rx, tx).await;
    let (_, tx) = h.streams(sender, None).await;
    assert_eq!(h.check(tx), 1);
}

#[tokio::test]
async fn network_streams_send_batches_and_filter_receive_sources() {
    let mut h = Harness::default();
    h.ctx.socket_addr_check =
        SocketAddrCheck::new(|addr, _| AddrDecision::allow_on(addr, Plane::Host));
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_addr = peer.local_addr().unwrap();
    let (socket, mut guest_addr) = h.bind("0.0.0.0:0").await;
    guest_addr.set_ip(peer_addr.ip());
    let (rx, tx) = h.streams(socket, Some(peer_addr)).await;
    assert!(h.check(tx) >= 3);
    let batch = (1..=3).map(|n| datagram(vec![n], None)).collect();
    assert_eq!(
        Outgoing::send(&mut h.view(), Resource::new_borrow(tx), batch).unwrap(),
        3
    );
    let mut buf = [0; 8];
    for expected in 1..=3 {
        let (n, addr) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], &[expected]);
        stranger.send_to(&[99], addr).await.unwrap();
    }
    peer.send_to(&[42], guest_addr).await.unwrap();
    h.ready(rx).await;
    let got = h.receive(rx, 10);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, [42]);
    assert_eq!(
        ip_socket_address_to_socket_addr(got[0].remote_address),
        peer_addr
    );
    h.drop_streams(rx, tx).await;
    let (rx, tx) = h.streams(socket, None).await;
    h.check(tx);
    h.send(tx, vec![7], Some(peer_addr)).unwrap();
    tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    stranger.send_to(&[99], guest_addr).await.unwrap();
    peer.send_to(&[43], guest_addr).await.unwrap();
    // Consume readiness until the allowed reply arrives; a stranger can wake the pollable.
    let got = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            h.ready(rx).await;
            let got = h.receive(rx, 10);
            if !got.is_empty() {
                break got;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, [43]);
}

#[tokio::test]
async fn policy_denies_create_bind_connect_and_send_without_delivery() {
    for denied_reason in [
        SocketAddrUse::UdpCreate,
        SocketAddrUse::UdpBind,
        SocketAddrUse::UdpConnect,
        SocketAddrUse::UdpOutgoingDatagram,
    ] {
        let mut h = Harness::default();
        h.ctx.socket_addr_check = SocketAddrCheck::new(move |addr, reason| {
            if std::mem::discriminant(&reason) == std::mem::discriminant(&denied_reason) {
                AddrDecision::Deny(DenyReason::NotPermitted)
            } else {
                AddrDecision::allow_by_address(addr)
            }
        });
        if matches!(denied_reason, SocketAddrUse::UdpCreate) {
            assert_code(
                udp_create_socket::Host::create_udp_socket(&mut h.view(), IpAddressFamily::Ipv4)
                    .await,
                ErrorCode::AccessDenied,
            );
            continue;
        }
        if matches!(denied_reason, SocketAddrUse::UdpBind) {
            let socket = h.create(IpAddressFamily::Ipv4).await;
            let network = instance_network::Host::instance_network(&mut h.view()).unwrap();
            assert_code(
                Socket::start_bind(
                    &mut h.view(),
                    Resource::new_borrow(socket),
                    network,
                    socket_addr_to_ip_socket_address("127.0.0.1:0".parse().unwrap()),
                )
                .await,
                ErrorCode::AccessDenied,
            );
            continue;
        }
        let (socket, _) = h.bind("127.0.0.1:0").await;
        let peer = "127.0.0.1:1234".parse().unwrap();
        if matches!(denied_reason, SocketAddrUse::UdpConnect) {
            assert_code(
                Socket::stream(
                    &mut h.view(),
                    Resource::new_borrow(socket),
                    Some(socket_addr_to_ip_socket_address(peer)),
                )
                .await,
                ErrorCode::AccessDenied,
            );
        } else {
            let (_, tx) = h.streams(socket, None).await;
            h.check(tx);
            assert_code(h.send(tx, vec![1], Some(peer)), ErrorCode::AccessDenied);
        }
    }
}

#[tokio::test]
async fn repeated_polling_preserves_buffered_datagrams() {
    for local in ["127.0.0.1:0", "0.0.0.0:0"] {
        let mut h = Harness::default();
        let (receiver, mut peer) = h.bind(local).await;
        peer.set_ip("127.0.0.1".parse().unwrap());
        let (sender, _) = h.bind("127.0.0.1:0").await;
        let (rx, _) = h.streams(receiver, None).await;
        let (_, tx) = h.streams(sender, None).await;
        for byte in [1, 2] {
            h.check(tx);
            h.send(tx, vec![byte], Some(peer)).unwrap();
        }
        h.ready(rx).await;
        h.ready(rx).await;
        assert_eq!(
            h.receive(rx, 10)
                .iter()
                .map(|d| d.data[0])
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }
}

#[tokio::test]
async fn unspecified_host_send_consumes_the_advertised_single_permit() {
    let mut h = Harness::default();
    h.ctx.socket_addr_check =
        SocketAddrCheck::new(|addr, _| AddrDecision::allow_on(addr, Plane::Host));
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (socket, _) = h.bind("0.0.0.0:0").await;
    let (_, tx) = h.streams(socket, None).await;
    assert_eq!(h.check(tx), 1);
    h.send(tx, vec![1], Some(peer.local_addr().unwrap()))
        .unwrap();
    assert!(
        h.send(tx, vec![2], Some(peer.local_addr().unwrap()))
            .unwrap_err()
            .downcast_ref()
            .is_none()
    );
}

#[tokio::test]
async fn socket_properties_and_pollables_follow_resource_lifetimes() {
    for local in ["127.0.0.1:0", "[::1]:0", "0.0.0.0:0", "[::]:0"] {
        let mut h = Harness::default();
        let (socket, addr) = h.bind(local).await;
        let family = Socket::address_family(&mut h.view(), Resource::new_borrow(socket)).unwrap();
        assert!(matches!(
            (family, addr),
            (IpAddressFamily::Ipv4, SocketAddr::V4(_)) | (IpAddressFamily::Ipv6, SocketAddr::V6(_))
        ));
        Socket::set_unicast_hop_limit(&mut h.view(), Resource::new_borrow(socket), 42).unwrap();
        assert_eq!(
            Socket::unicast_hop_limit(&mut h.view(), Resource::new_borrow(socket)).unwrap(),
            42
        );
        assert_code(
            Socket::set_unicast_hop_limit(&mut h.view(), Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        for size in [4096, 8192] {
            Socket::set_receive_buffer_size(&mut h.view(), Resource::new_borrow(socket), size)
                .unwrap();
            Socket::set_send_buffer_size(&mut h.view(), Resource::new_borrow(socket), size)
                .unwrap();
            assert!(
                Socket::receive_buffer_size(&mut h.view(), Resource::new_borrow(socket)).unwrap()
                    >= size
            );
            assert!(
                Socket::send_buffer_size(&mut h.view(), Resource::new_borrow(socket)).unwrap()
                    >= size
            );
        }
        assert_code(
            Socket::set_receive_buffer_size(&mut h.view(), Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        assert_code(
            Socket::set_send_buffer_size(&mut h.view(), Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        let socket_poll = Socket::subscribe(&mut h.view(), Resource::new_borrow(socket)).unwrap();
        let (rx, tx) = h.streams(socket, None).await;
        let rx_poll = Incoming::subscribe(&mut h.view(), Resource::new_borrow(rx)).unwrap();
        let tx_poll = Outgoing::subscribe(&mut h.view(), Resource::new_borrow(tx)).unwrap();
        assert!(Incoming::drop(&mut h.view(), Resource::new_own(rx)).is_err());
        assert!(
            Outgoing::drop(&mut h.view(), Resource::new_own(tx))
                .await
                .is_err()
        );
        h.table.delete(rx_poll).unwrap();
        h.table.delete(tx_poll).unwrap();
        h.drop_streams(rx, tx).await;
        assert!(Socket::drop(&mut h.view(), Resource::new_own(socket)).is_err());
        h.table.delete(socket_poll).unwrap();
        Socket::drop(&mut h.view(), Resource::new_own(socket)).unwrap();
    }
}

#[tokio::test]
async fn outgoing_readiness_tracks_send_buffer_capacity() {
    for local in ["127.0.0.1:0", "0.0.0.0:0"] {
        let mut h = Harness::default();
        let (receiver, peer) = h.bind("127.0.0.1:0").await;
        let (sender, _) = h.bind(local).await;
        Socket::set_send_buffer_size(&mut h.view(), Resource::new_borrow(sender), 2).unwrap();
        let (rx, _) = h.streams(receiver, None).await;
        let (_, tx) = h.streams(sender, None).await;
        for _ in 0..2 {
            let stream = h
                .table
                .get_mut(&Resource::<OutgoingDatagramStream>::new_borrow(tx))
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), stream.ready())
                .await
                .unwrap();
            assert_eq!(h.check(tx), 1);
        }
        h.send(tx, vec![1; 2], Some(peer)).unwrap();
        {
            let stream = h
                .table
                .get_mut(&Resource::<OutgoingDatagramStream>::new_borrow(tx))
                .unwrap();
            let mut ready = Box::pin(stream.ready());
            assert!(futures::poll!(ready.as_mut()).is_pending());
        }
        assert_eq!(h.receive(rx, 1).len(), 1);
        let stream = h
            .table
            .get_mut(&Resource::<OutgoingDatagramStream>::new_borrow(tx))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), stream.ready())
            .await
            .unwrap();
        assert_eq!(h.check(tx), 1);
    }
}
