#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::sockets::{AddrDecision, DenyReason, Plane, SocketAddrCheck, WasiSocketsCtx};
use std::time::Duration;

struct Data {
    ctx: WasiSocketsCtx,
    table: ResourceTable,
}

impl Default for Data {
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

fn view(data: &mut Data) -> WasiSocketsCtxView<'_> {
    WasiSocketsCtxView {
        ctx: &mut data.ctx,
        table: &mut data.table,
    }
}

fn store(data: Data) -> wasmtime::Store<Data> {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model_async(true);
    wasmtime::Store::new(&wasmtime::Engine::new(&config).unwrap(), data)
}

async fn create(data: &mut Data, family: IpAddressFamily) -> u32 {
    HostUdpSocket::create(&mut view(data), family)
        .await
        .unwrap()
        .rep()
}

async fn bind(data: &mut Data, local: &str) -> (u32, SocketAddr) {
    let addr: SocketAddr = local.parse().unwrap();
    let socket = create(
        data,
        if addr.is_ipv4() {
            IpAddressFamily::Ipv4
        } else {
            IpAddressFamily::Ipv6
        },
    )
    .await;
    HostUdpSocket::bind(&mut view(data), Resource::new_borrow(socket), addr.into())
        .await
        .unwrap();
    let addr =
        HostUdpSocket::get_local_address(&mut view(data), Resource::new_borrow(socket)).unwrap();
    (socket, addr.into())
}

async fn send(
    store: &mut wasmtime::Store<Data>,
    rep: u32,
    data: Vec<u8>,
    remote: Option<SocketAddr>,
) -> SocketResult<()> {
    store
        .run_concurrent(async |accessor| {
            let accessor = accessor.with_getter::<WasiSockets>(view);
            <WasiSockets as HostUdpSocketWithStore<Data>>::send(
                &accessor,
                Resource::new_borrow(rep),
                data,
                remote.map(Into::into),
            )
            .await
        })
        .await
        .unwrap()
}

async fn receive(
    store: &mut wasmtime::Store<Data>,
    rep: u32,
) -> SocketResult<(Vec<u8>, IpSocketAddress)> {
    tokio::time::timeout(
        Duration::from_secs(2),
        store.run_concurrent(async |accessor| {
            let accessor = accessor.with_getter::<WasiSockets>(view);
            <WasiSockets as HostUdpSocketWithStore<Data>>::receive(
                &accessor,
                Resource::new_borrow(rep),
            )
            .await
        }),
    )
    .await
    .expect("UDP receive timed out")
    .unwrap()
}

fn assert_code<T: std::fmt::Debug>(result: SocketResult<T>, expected: ErrorCode) {
    let error = result.unwrap_err();
    assert_eq!(
        std::mem::discriminant(error.downcast_ref().expect("socket error code")),
        std::mem::discriminant(&expected),
        "{error:?}"
    );
}

#[tokio::test]
async fn virtual_datagrams_roundtrip_for_both_families_and_connection_modes() {
    for local in ["127.0.0.1:0", "[::1]:0"] {
        for connected in [false, true] {
            let mut data = Data::default();
            let (receiver, peer) = bind(&mut data, local).await;
            let (sender, source) = bind(&mut data, local).await;
            HostUdpSocket::set_send_buffer_size(
                &mut view(&mut data),
                Resource::new_borrow(sender),
                MAX_UDP_DATAGRAM_SIZE as u64,
            )
            .unwrap();
            if connected {
                HostUdpSocket::connect(
                    &mut view(&mut data),
                    Resource::new_borrow(sender),
                    peer.into(),
                )
                .await
                .unwrap();
            }
            let mut store = store(data);
            for payload in [vec![], vec![0, 255, 1], vec![7; MAX_UDP_DATAGRAM_SIZE]] {
                send(
                    &mut store,
                    sender,
                    payload.clone(),
                    (!connected).then_some(peer),
                )
                .await
                .unwrap();
                let (got, addr) = receive(&mut store, receiver).await.unwrap();
                assert_eq!(got, payload);
                assert_eq!(SocketAddr::from(addr), source);
            }
        }
    }
}

#[tokio::test]
async fn send_to_implicitly_binds_once_and_routes_virtual_traffic() {
    let mut data = Data::default();
    let (receiver, peer) = bind(&mut data, "127.0.0.1:0").await;
    let sender = create(&mut data, IpAddressFamily::Ipv4).await;
    let mut store = store(data);
    let mut source = None;
    for byte in [1, 2] {
        send(&mut store, sender, vec![byte], Some(peer))
            .await
            .unwrap();
        let (payload, addr) = receive(&mut store, receiver).await.unwrap();
        let addr = SocketAddr::from(addr);
        assert_eq!(payload, [byte]);
        assert_ne!(addr.port(), 0);
        assert!(addr.ip().is_loopback());
        assert_eq!(*source.get_or_insert(addr), addr);
    }
    let local =
        HostUdpSocket::get_local_address(&mut view(store.data_mut()), Resource::new_borrow(sender))
            .unwrap();
    assert_eq!(SocketAddr::from(local).port(), source.unwrap().port());
}

#[tokio::test]
async fn send_rejects_missing_destination_and_oversized_datagram() {
    let mut data = Data::default();
    let unbound = create(&mut data, IpAddressFamily::Ipv4).await;
    let (bound, peer) = bind(&mut data, "127.0.0.1:0").await;
    let mut store = store(data);
    for rep in [unbound, bound] {
        assert_code(
            send(&mut store, rep, vec![1], None).await,
            ErrorCode::InvalidArgument,
        );
        assert_code(
            send(
                &mut store,
                rep,
                vec![0; MAX_UDP_DATAGRAM_SIZE + 1],
                Some(peer),
            )
            .await,
            ErrorCode::DatagramTooLarge,
        );
    }
}

#[tokio::test]
async fn virtual_socket_rejects_host_plane_destination() {
    let mut data = Data::default();
    let (socket, _) = bind(&mut data, "127.0.0.1:0").await;
    data.ctx.socket_addr_check =
        SocketAddrCheck::new(|addr, _| AddrDecision::allow_on(addr, Plane::Host));
    let mut store = store(data);
    assert_code(
        send(
            &mut store,
            socket,
            vec![1],
            Some("127.0.0.1:9".parse().unwrap()),
        )
        .await,
        ErrorCode::InvalidArgument,
    );
}

#[tokio::test]
async fn receive_skips_denied_datagrams_and_reports_refusal_limit() {
    let mut data = Data::default();
    let (receiver, peer) = bind(&mut data, "127.0.0.1:0").await;
    let (denied, denied_addr) = bind(&mut data, "127.0.0.1:0").await;
    let (allowed, allowed_addr) = bind(&mut data, "127.0.0.1:0").await;
    data.ctx.socket_addr_check = SocketAddrCheck::new(move |addr, reason| {
        if matches!(reason, SocketAddrUse::UdpReceive) && addr == denied_addr {
            AddrDecision::Deny(DenyReason::NotPermitted)
        } else {
            AddrDecision::allow_by_address(addr)
        }
    });
    let mut store = store(data);
    send(&mut store, denied, vec![1], Some(peer)).await.unwrap();
    send(&mut store, allowed, vec![2], Some(peer))
        .await
        .unwrap();
    let (payload, addr) = receive(&mut store, receiver).await.unwrap();
    assert_eq!(payload, [2]);
    assert_eq!(SocketAddr::from(addr), allowed_addr);
    for _ in 0..MAX_REFUSED_DATAGRAMS_PER_RECEIVE {
        send(&mut store, denied, vec![1], Some(peer)).await.unwrap();
    }
    assert_code(receive(&mut store, receiver).await, ErrorCode::AccessDenied);
}

#[tokio::test]
async fn denied_implicit_bind_leaves_socket_unbound() {
    let mut data = Data::default();
    let socket = create(&mut data, IpAddressFamily::Ipv4).await;
    data.ctx.socket_addr_check = SocketAddrCheck::new(|addr, reason| {
        if matches!(reason, SocketAddrUse::UdpImplicitBind) {
            AddrDecision::Deny(DenyReason::BindNotPermitted)
        } else {
            AddrDecision::allow_by_address(addr)
        }
    });
    let mut store = store(data);
    assert_code(
        send(
            &mut store,
            socket,
            vec![1],
            Some("127.0.0.1:9".parse().unwrap()),
        )
        .await,
        ErrorCode::AccessDenied,
    );
    let sock = store
        .data()
        .table
        .get(&Resource::<UdpSocket>::new_borrow(socket))
        .unwrap();
    assert!(sock.needs_implicit_bind());
}

#[tokio::test]
async fn host_plane_roundtrip_admits_reply_and_filters_strangers() {
    let mut data = Data::default();
    data.ctx.socket_addr_check =
        SocketAddrCheck::new(|addr, _| AddrDecision::allow_on(addr, Plane::Host));
    let socket = create(&mut data, IpAddressFamily::Ipv4).await;
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut store = store(data);
    send(
        &mut store,
        socket,
        vec![1],
        Some(peer.local_addr().unwrap()),
    )
    .await
    .unwrap();
    let mut buf = [0; 8];
    let (n, guest) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], &[1]);
    stranger.send_to(&[99], guest).await.unwrap();
    peer.send_to(&[42], guest).await.unwrap();
    let (payload, addr) = receive(&mut store, socket).await.unwrap();
    assert_eq!(payload, [42]);
    assert_eq!(SocketAddr::from(addr), peer.local_addr().unwrap());
}

#[tokio::test]
async fn options_addresses_disconnect_and_drop_use_guest_resources() {
    for local in ["127.0.0.1:0", "[::1]:0", "0.0.0.0:0", "[::]:0"] {
        let mut data = Data::default();
        let (socket, addr) = bind(&mut data, local).await;
        let mut v = view(&mut data);
        let family =
            HostUdpSocket::get_address_family(&mut v, Resource::new_borrow(socket)).unwrap();
        assert!(matches!(
            (family, addr),
            (IpAddressFamily::Ipv4, SocketAddr::V4(_)) | (IpAddressFamily::Ipv6, SocketAddr::V6(_))
        ));
        HostUdpSocket::set_unicast_hop_limit(&mut v, Resource::new_borrow(socket), 42).unwrap();
        assert_eq!(
            HostUdpSocket::get_unicast_hop_limit(&mut v, Resource::new_borrow(socket)).unwrap(),
            42
        );
        assert_code(
            HostUdpSocket::set_unicast_hop_limit(&mut v, Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        for size in [4096, 8192] {
            HostUdpSocket::set_receive_buffer_size(&mut v, Resource::new_borrow(socket), size)
                .unwrap();
            HostUdpSocket::set_send_buffer_size(&mut v, Resource::new_borrow(socket), size)
                .unwrap();
            assert!(
                HostUdpSocket::get_receive_buffer_size(&mut v, Resource::new_borrow(socket))
                    .unwrap()
                    >= size
            );
            assert!(
                HostUdpSocket::get_send_buffer_size(&mut v, Resource::new_borrow(socket)).unwrap()
                    >= size
            );
        }
        assert_code(
            HostUdpSocket::set_receive_buffer_size(&mut v, Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        assert_code(
            HostUdpSocket::set_send_buffer_size(&mut v, Resource::new_borrow(socket), 0),
            ErrorCode::InvalidArgument,
        );
        let peer: SocketAddr = if addr.is_ipv4() {
            "127.0.0.1:1234"
        } else {
            "[::1]:1234"
        }
        .parse()
        .unwrap();
        HostUdpSocket::connect(&mut v, Resource::new_borrow(socket), peer.into())
            .await
            .unwrap();
        assert_eq!(
            SocketAddr::from(
                HostUdpSocket::get_remote_address(&mut v, Resource::new_borrow(socket)).unwrap()
            ),
            peer
        );
        HostUdpSocket::disconnect(&mut v, Resource::new_borrow(socket)).unwrap();
        assert_code(
            HostUdpSocket::get_remote_address(&mut v, Resource::new_borrow(socket)),
            ErrorCode::InvalidState,
        );
        HostUdpSocket::drop(&mut v, Resource::new_own(socket)).unwrap();
        assert!(
            HostUdpSocket::get_local_address(&mut v, Resource::new_borrow(socket))
                .unwrap_err()
                .downcast_ref()
                .is_none()
        );
    }
}

#[tokio::test]
async fn invalid_virtual_destination_is_rejected_before_delivery() {
    let mut data = Data::default();
    let (sender, _) = bind(&mut data, "127.0.0.1:0").await;
    let mut store = store(data);
    for destination in ["127.0.0.1:0", "0.0.0.0:9", "[::1]:9"] {
        assert_code(
            send(
                &mut store,
                sender,
                vec![1],
                Some(destination.parse().unwrap()),
            )
            .await,
            ErrorCode::InvalidArgument,
        );
    }
}

#[tokio::test]
async fn connected_receive_discards_queued_datagrams_from_previous_peers() {
    let mut data = Data::default();
    let (receiver, destination) = bind(&mut data, "127.0.0.1:0").await;
    let (stranger, _) = bind(&mut data, "127.0.0.1:0").await;
    let (peer, peer_addr) = bind(&mut data, "127.0.0.1:0").await;
    let mut store = store(data);
    send(&mut store, stranger, vec![99], Some(destination))
        .await
        .unwrap();
    send(&mut store, peer, vec![42], Some(destination))
        .await
        .unwrap();
    HostUdpSocket::connect(
        &mut view(store.data_mut()),
        Resource::new_borrow(receiver),
        peer_addr.into(),
    )
    .await
    .unwrap();
    let (got, addr) = receive(&mut store, receiver).await.unwrap();
    assert_eq!(got, [42]);
    assert_eq!(SocketAddr::from(addr), peer_addr);
}

#[tokio::test]
async fn unspecified_receive_accepts_virtual_datagrams_and_host_replies() {
    let mut data = Data::default();
    let (receiver, mut destination) = bind(&mut data, "0.0.0.0:0").await;
    destination.set_ip("127.0.0.1".parse().unwrap());
    let (sender, source) = bind(&mut data, "127.0.0.1:0").await;
    let mut store = store(data);
    send(&mut store, sender, vec![42], Some(destination))
        .await
        .unwrap();
    let (got, addr) = receive(&mut store, receiver).await.unwrap();
    assert_eq!(got, [42]);
    assert_eq!(SocketAddr::from(addr), source);

    let host_peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let host_addr = host_peer.local_addr().unwrap();
    store.data_mut().ctx.socket_addr_check = SocketAddrCheck::new(move |addr, _| {
        if addr == host_addr {
            AddrDecision::allow_on(addr, Plane::Host)
        } else {
            AddrDecision::allow_by_address(addr)
        }
    });
    send(&mut store, receiver, vec![7], Some(host_addr))
        .await
        .unwrap();
    let mut buf = [0; 8];
    let (n, guest) = tokio::time::timeout(Duration::from_secs(2), host_peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], &[7]);
    host_peer.send_to(&[8], guest).await.unwrap();
    let (got, addr) = receive(&mut store, receiver).await.unwrap();
    assert_eq!(got, [8]);
    assert_eq!(SocketAddr::from(addr), host_addr);
}

#[tokio::test]
async fn independent_virtual_sockets_do_not_share_send_capacity() {
    let mut data = Data::default();
    let (receiver_a, peer_a) = bind(&mut data, "127.0.0.1:0").await;
    let (receiver_b, peer_b) = bind(&mut data, "127.0.0.1:0").await;
    let (sender_a, _) = bind(&mut data, "127.0.0.1:0").await;
    let (sender_b, _) = bind(&mut data, "127.0.0.1:0").await;
    for sender in [sender_a, sender_b] {
        HostUdpSocket::set_send_buffer_size(
            &mut view(&mut data),
            Resource::new_borrow(sender),
            MAX_UDP_DATAGRAM_SIZE as u64,
        )
        .unwrap();
    }
    let mut store = store(data);
    send(
        &mut store,
        sender_a,
        vec![1; MAX_UDP_DATAGRAM_SIZE],
        Some(peer_a),
    )
    .await
    .unwrap();
    send(
        &mut store,
        sender_b,
        vec![2; MAX_UDP_DATAGRAM_SIZE],
        Some(peer_b),
    )
    .await
    .unwrap();
    assert_eq!(
        receive(&mut store, receiver_a).await.unwrap().0,
        vec![1; MAX_UDP_DATAGRAM_SIZE]
    );
    assert_eq!(
        receive(&mut store, receiver_b).await.unwrap().0,
        vec![2; MAX_UDP_DATAGRAM_SIZE]
    );
}

#[tokio::test]
async fn datagram_larger_than_the_send_buffer_is_too_large() {
    let mut data = Data::default();
    let (receiver, peer) = bind(&mut data, "127.0.0.1:0").await;
    let (sender, _) = bind(&mut data, "127.0.0.1:0").await;
    HostUdpSocket::set_send_buffer_size(&mut view(&mut data), Resource::new_borrow(sender), 2)
        .unwrap();
    let mut store = store(data);
    assert_code(
        send(&mut store, sender, vec![1; 3], Some(peer)).await,
        ErrorCode::DatagramTooLarge,
    );
    send(&mut store, sender, vec![1; 2], Some(peer))
        .await
        .unwrap();
    assert_eq!(receive(&mut store, receiver).await.unwrap().0, [1, 1]);
}

#[tokio::test]
async fn full_receive_buffer_drops_without_pinning_the_sender() {
    let mut data = Data::default();
    let (idle, idle_addr) = bind(&mut data, "127.0.0.1:0").await;
    let (healthy, healthy_addr) = bind(&mut data, "127.0.0.1:0").await;
    let (sender, _) = bind(&mut data, "127.0.0.1:0").await;
    HostUdpSocket::set_receive_buffer_size(&mut view(&mut data), Resource::new_borrow(idle), 2)
        .unwrap();
    let mut store = store(data);
    send(&mut store, sender, vec![1; 2], Some(idle_addr))
        .await
        .unwrap();
    send(&mut store, sender, vec![2], Some(idle_addr))
        .await
        .unwrap();
    send(&mut store, sender, vec![3], Some(healthy_addr))
        .await
        .unwrap();
    assert_eq!(receive(&mut store, healthy).await.unwrap().0, [3]);
    assert_eq!(receive(&mut store, idle).await.unwrap().0, [1, 1]);
    send(&mut store, sender, vec![4], Some(idle_addr))
        .await
        .unwrap();
    assert_eq!(receive(&mut store, idle).await.unwrap().0, [4]);
}

#[tokio::test]
async fn malformed_destination_is_rejected_before_the_policy() {
    let mut data = Data::default();
    let (sender, _) = bind(&mut data, "127.0.0.1:0").await;
    data.ctx.socket_addr_check = SocketAddrCheck::new(|addr, reason| {
        if matches!(reason, SocketAddrUse::UdpOutgoingDatagram) {
            AddrDecision::Deny(DenyReason::NotPermitted)
        } else {
            AddrDecision::allow_by_address(addr)
        }
    });
    let mut store = store(data);
    for destination in ["127.0.0.1:0", "0.0.0.0:9"] {
        assert_code(
            send(
                &mut store,
                sender,
                vec![1],
                Some(destination.parse().unwrap()),
            )
            .await,
            ErrorCode::InvalidArgument,
        );
    }
}

#[tokio::test]
async fn connected_v6_receive_ignores_flow_label_and_scope() {
    let mut data = Data::default();
    let (receiver, destination) = bind(&mut data, "[::1]:0").await;
    let (sender, source) = bind(&mut data, "[::1]:0").await;
    let SocketAddr::V6(source) = source else {
        unreachable!("bound a v6 socket")
    };
    let labelled = std::net::SocketAddrV6::new(*source.ip(), source.port(), 7, 0);
    HostUdpSocket::connect(
        &mut view(&mut data),
        Resource::new_borrow(receiver),
        SocketAddr::V6(labelled).into(),
    )
    .await
    .unwrap();
    let mut store = store(data);
    send(&mut store, sender, vec![5], Some(destination))
        .await
        .unwrap();
    assert_eq!(receive(&mut store, receiver).await.unwrap().0, [5]);
}

#[tokio::test]
async fn connected_unspecified_receive_uses_the_connected_plane() {
    let mut data = Data::default();
    let (receiver, mut destination) = bind(&mut data, "0.0.0.0:0").await;
    destination.set_ip("127.0.0.1".parse().unwrap());
    let (sender, source) = bind(&mut data, "127.0.0.1:0").await;
    HostUdpSocket::connect(
        &mut view(&mut data),
        Resource::new_borrow(receiver),
        source.into(),
    )
    .await
    .unwrap();
    let mut store = store(data);
    send(&mut store, sender, vec![1], Some(destination))
        .await
        .unwrap();
    assert_eq!(receive(&mut store, receiver).await.unwrap().0, [1]);
    HostUdpSocket::disconnect(&mut view(store.data_mut()), Resource::new_borrow(receiver)).unwrap();

    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_addr = peer.local_addr().unwrap();
    store.data_mut().ctx.socket_addr_check =
        SocketAddrCheck::new(|addr, _| AddrDecision::allow_on(addr, Plane::Host));
    HostUdpSocket::connect(
        &mut view(store.data_mut()),
        Resource::new_borrow(receiver),
        peer_addr.into(),
    )
    .await
    .unwrap();
    peer.send_to(&[2], destination).await.unwrap();
    assert_eq!(receive(&mut store, receiver).await.unwrap().0, [2]);
}

#[tokio::test]
async fn policy_denies_each_p3_socket_operation() {
    for denied_reason in [
        SocketAddrUse::UdpCreate,
        SocketAddrUse::UdpBind,
        SocketAddrUse::UdpConnect,
        SocketAddrUse::UdpOutgoingDatagram,
    ] {
        let mut data = Data::default();
        data.ctx.socket_addr_check = SocketAddrCheck::new(move |addr, reason| {
            if std::mem::discriminant(&reason) == std::mem::discriminant(&denied_reason) {
                AddrDecision::Deny(DenyReason::NotPermitted)
            } else {
                AddrDecision::allow_by_address(addr)
            }
        });
        if matches!(denied_reason, SocketAddrUse::UdpCreate) {
            assert_code(
                HostUdpSocket::create(&mut view(&mut data), IpAddressFamily::Ipv4).await,
                ErrorCode::AccessDenied,
            );
            continue;
        }
        if matches!(denied_reason, SocketAddrUse::UdpBind) {
            let socket = create(&mut data, IpAddressFamily::Ipv4).await;
            assert_code(
                HostUdpSocket::bind(
                    &mut view(&mut data),
                    Resource::new_borrow(socket),
                    "127.0.0.1:0".parse::<SocketAddr>().unwrap().into(),
                )
                .await,
                ErrorCode::AccessDenied,
            );
            continue;
        }
        let (socket, _) = bind(&mut data, "127.0.0.1:0").await;
        let peer: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        if matches!(denied_reason, SocketAddrUse::UdpConnect) {
            assert_code(
                HostUdpSocket::connect(
                    &mut view(&mut data),
                    Resource::new_borrow(socket),
                    peer.into(),
                )
                .await,
                ErrorCode::AccessDenied,
            );
        } else {
            assert_code(
                send(&mut store(data), socket, vec![1], Some(peer)).await,
                ErrorCode::AccessDenied,
            );
        }
    }
}

#[tokio::test]
async fn pending_receive_follows_reconnected_peer() {
    let mut data = Data::default();
    let (receiver, destination) = bind(&mut data, "127.0.0.1:0").await;
    let (_, old_peer) = bind(&mut data, "127.0.0.1:0").await;
    let (sender, new_peer) = bind(&mut data, "127.0.0.1:0").await;
    HostUdpSocket::connect(
        &mut view(&mut data),
        Resource::new_borrow(receiver),
        old_peer.into(),
    )
    .await
    .unwrap();
    let mut store = store(data);
    store
        .run_concurrent(async |accessor| {
            let accessor = accessor.with_getter::<WasiSockets>(view);
            let mut pending = Box::pin(<WasiSockets as HostUdpSocketWithStore<Data>>::receive(
                &accessor,
                Resource::new_borrow(receiver),
            ));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            accessor.with(|mut a| {
                let mut v = a.get();
                futures::executor::block_on(HostUdpSocket::connect(
                    &mut v,
                    Resource::new_borrow(receiver),
                    new_peer.into(),
                ))
                .unwrap();
            });
            <WasiSockets as HostUdpSocketWithStore<Data>>::send(
                &accessor,
                Resource::new_borrow(sender),
                vec![42],
                Some(destination.into()),
            )
            .await
            .unwrap();
            let received = tokio::time::timeout(Duration::from_secs(2), pending).await;
            assert!(
                received.is_ok(),
                "pending receive discarded the new connected peer's datagram"
            );
            assert_eq!(received.unwrap().unwrap().0, vec![42]);
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn pending_receive_follows_connected_plane_changes() {
    for start_virtual in [true, false] {
        let mut data = Data::default();
        let (receiver, mut destination) = bind(&mut data, "0.0.0.0:0").await;
        destination.set_ip("127.0.0.1".parse().unwrap());
        let (sender, virtual_peer) = bind(&mut data, "127.0.0.1:0").await;
        let host = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let host_peer = host.local_addr().unwrap();
        data.ctx.socket_addr_check = SocketAddrCheck::new(move |addr, _| {
            if addr == host_peer {
                AddrDecision::allow_on(addr, Plane::Host)
            } else {
                AddrDecision::allow_by_address(addr)
            }
        });
        let (old_peer, new_peer) = if start_virtual {
            (virtual_peer, host_peer)
        } else {
            (host_peer, virtual_peer)
        };
        HostUdpSocket::connect(
            &mut view(&mut data),
            Resource::new_borrow(receiver),
            old_peer.into(),
        )
        .await
        .unwrap();
        let mut store = store(data);
        store
            .run_concurrent(async |accessor| {
                let accessor = accessor.with_getter::<WasiSockets>(view);
                let mut pending = Box::pin(<WasiSockets as HostUdpSocketWithStore<Data>>::receive(
                    &accessor,
                    Resource::new_borrow(receiver),
                ));
                assert!(futures::poll!(pending.as_mut()).is_pending());
                accessor.with(|mut a| {
                    futures::executor::block_on(HostUdpSocket::connect(
                        &mut a.get(),
                        Resource::new_borrow(receiver),
                        new_peer.into(),
                    ))
                    .unwrap();
                });
                if start_virtual {
                    host.send_to(&[42], destination).await.unwrap();
                } else {
                    <WasiSockets as HostUdpSocketWithStore<Data>>::send(
                        &accessor,
                        Resource::new_borrow(sender),
                        vec![42],
                        Some(destination.into()),
                    )
                    .await
                    .unwrap();
                }
                let (payload, source) = tokio::time::timeout(Duration::from_secs(2), pending)
                    .await
                    .expect("receive should follow the new plane")
                    .unwrap();
                assert_eq!(payload, [42]);
                assert_eq!(SocketAddr::from(source), new_peer);
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn pending_receive_accepts_other_peers_after_disconnect() {
    let mut data = Data::default();
    let (receiver, destination) = bind(&mut data, "127.0.0.1:0").await;
    let (_, old_peer) = bind(&mut data, "127.0.0.1:0").await;
    let (sender, _) = bind(&mut data, "127.0.0.1:0").await;
    HostUdpSocket::connect(
        &mut view(&mut data),
        Resource::new_borrow(receiver),
        old_peer.into(),
    )
    .await
    .unwrap();
    let mut store = store(data);
    store
        .run_concurrent(async |accessor| {
            let accessor = accessor.with_getter::<WasiSockets>(view);
            let mut pending = Box::pin(<WasiSockets as HostUdpSocketWithStore<Data>>::receive(
                &accessor,
                Resource::new_borrow(receiver),
            ));
            assert!(futures::poll!(pending.as_mut()).is_pending());
            accessor.with(|mut a| {
                HostUdpSocket::disconnect(&mut a.get(), Resource::new_borrow(receiver)).unwrap();
            });
            <WasiSockets as HostUdpSocketWithStore<Data>>::send(
                &accessor,
                Resource::new_borrow(sender),
                vec![42],
                Some(destination.into()),
            )
            .await
            .unwrap();
            let received = tokio::time::timeout(Duration::from_secs(2), pending)
                .await
                .expect("receive should follow disconnect")
                .unwrap();
            assert_eq!(received.0, [42]);
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn p2_and_p3_senders_share_the_receivers_buffer_budget() {
    use wasmtime_wasi::p2::bindings::sockets::{
        instance_network,
        network::IpAddressFamily as P2Family,
        udp::{
            HostIncomingDatagramStream as P2Incoming, HostOutgoingDatagramStream as P2Outgoing,
            HostUdpSocket as P2Socket, OutgoingDatagram,
        },
        udp_create_socket,
    };
    for p2_first in [true, false] {
        let mut data = Data::default();
        let (receiver, destination) = bind(&mut data, "127.0.0.1:0").await;
        let p2_sender =
            udp_create_socket::Host::create_udp_socket(&mut view(&mut data), P2Family::Ipv4)
                .await
                .unwrap()
                .rep();
        let network = instance_network::Host::instance_network(&mut view(&mut data)).unwrap();
        P2Socket::start_bind(
            &mut view(&mut data),
            Resource::new_borrow(p2_sender),
            network,
            crate::sockets::host_network::socket_addr_to_ip_socket_address(
                "127.0.0.1:0".parse().unwrap(),
            ),
        )
        .await
        .unwrap();
        P2Socket::finish_bind(&mut view(&mut data), Resource::new_borrow(p2_sender)).unwrap();
        let (p3_sender, _) = bind(&mut data, "127.0.0.1:0").await;
        HostUdpSocket::set_receive_buffer_size(
            &mut view(&mut data),
            Resource::new_borrow(receiver),
            1,
        )
        .unwrap();
        let (_, p2_tx) =
            P2Socket::stream(&mut view(&mut data), Resource::new_borrow(p2_sender), None)
                .await
                .unwrap();
        let mut store = store(data);
        let p2_send = |data: &mut Data, byte| {
            let mut v = view(data);
            assert_eq!(
                P2Outgoing::check_send(&mut v, Resource::new_borrow(p2_tx.rep())).unwrap(),
                1
            );
            assert_eq!(
                P2Outgoing::send(
                    &mut v,
                    Resource::new_borrow(p2_tx.rep()),
                    vec![OutgoingDatagram {
                        data: vec![byte],
                        remote_address: Some(
                            crate::sockets::host_network::socket_addr_to_ip_socket_address(
                                destination
                            )
                        ),
                    }]
                )
                .unwrap(),
                1
            );
        };
        if p2_first {
            p2_send(store.data_mut(), 1);
            send(&mut store, p3_sender, vec![2], Some(destination))
                .await
                .unwrap();
        } else {
            send(&mut store, p3_sender, vec![1], Some(destination))
                .await
                .unwrap();
            p2_send(store.data_mut(), 2);
        }
        assert_eq!(receive(&mut store, receiver).await.unwrap().0, [1]);
        // P2 can inspect the same receiver queue without waiting: the second
        // datagram must have been dropped, regardless of which API sent it.
        let (rx, _) = P2Socket::stream(
            &mut view(store.data_mut()),
            Resource::new_borrow(receiver),
            None,
        )
        .await
        .unwrap();
        assert!(
            P2Incoming::receive(
                &mut view(store.data_mut()),
                Resource::new_borrow(rx.rep()),
                10
            )
            .unwrap()
            .is_empty()
        );
        p2_send(store.data_mut(), 3);
        assert_eq!(receive(&mut store, receiver).await.unwrap().0, [3]);
    }
}
