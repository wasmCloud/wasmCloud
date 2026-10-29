#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::sockets::loopback::Network;
use crate::sockets::{AddrDecision, Plane};

const FAMILIES: [(SocketAddressFamily, &str, &str); 2] = [
    (SocketAddressFamily::Ipv4, "127.0.0.1:0", "0.0.0.0:0"),
    (SocketAddressFamily::Ipv6, "[::1]:0", "[::]:0"),
];

async fn socket(family: SocketAddressFamily) -> UdpSocket {
    UdpSocket::new(&WasiSocketsCtx::default(), family)
        .await
        .unwrap()
}

#[tokio::test]
async fn disabled_udp_rejects_creation_in_both_families() {
    let mut ctx = WasiSocketsCtx::default();
    ctx.allowed_network_uses.udp = false;
    for (family, _, _) in FAMILIES {
        assert!(matches!(
            UdpSocket::new(&ctx, family).await,
            Err(ErrorCode::AccessDenied)
        ));
    }
}

#[tokio::test]
async fn bind_lifecycle_and_drop_release_virtual_ports() {
    for (family, local, unspecified) in FAMILIES {
        for address in [local, unspecified] {
            let mut net = Network::default();
            let mut sock = socket(family).await;
            assert_eq!(sock.address_family(), family);
            assert!(sock.needs_implicit_bind());
            assert!(matches!(sock.finish_bind(), Err(ErrorCode::NotInProgress)));
            assert!(matches!(
                sock.disconnect(&mut net),
                Err(ErrorCode::InvalidState)
            ));
            assert!(matches!(sock.local_address(), Err(ErrorCode::InvalidState)));
            assert!(matches!(
                sock.remote_address(),
                Err(ErrorCode::InvalidState)
            ));
            sock.bind(address.parse().unwrap(), &mut net).unwrap();
            assert!(!sock.needs_implicit_bind());
            assert!(!sock.is_bound());
            assert!(matches!(sock.local_address(), Err(ErrorCode::InvalidState)));
            assert!(matches!(
                sock.bind(address.parse().unwrap(), &mut net),
                Err(ErrorCode::InvalidState)
            ));
            sock.finish_bind().unwrap();
            assert!(sock.is_bound());
            assert!(!sock.is_connected());
            assert!(matches!(sock.finish_bind(), Err(ErrorCode::NotInProgress)));
            let bound = sock.local_address().unwrap();
            assert_ne!(bound.port(), 0);
            assert_eq!(bound.ip(), address.parse::<SocketAddr>().unwrap().ip());
            let mut virtual_addr: SocketAddr = local.parse().unwrap();
            virtual_addr.set_port(bound.port());
            assert!(matches!(
                net.bind_udp(virtual_addr),
                Err(ErrorCode::AddressInUse)
            ));
            sock.drop(&mut net).unwrap();
            net.bind_udp(virtual_addr).unwrap();
        }
    }
}

#[tokio::test]
async fn drop_during_bind_releases_registration() {
    let mut net = Network::default();
    let mut sock = socket(SocketAddressFamily::Ipv4).await;
    let addr = "127.0.0.1:1234".parse().unwrap();
    sock.bind(addr, &mut net).unwrap();
    sock.drop(&mut net).unwrap();
    net.bind_udp(addr).unwrap();
}

#[tokio::test]
async fn invalid_bind_does_not_consume_socket() {
    for (family, local, _) in FAMILIES {
        let mut net = Network::default();
        let mut sock = socket(family).await;
        let wrong = if family == SocketAddressFamily::Ipv4 {
            "[::1]:0"
        } else {
            "127.0.0.1:0"
        };
        assert!(matches!(
            sock.bind(wrong.parse().unwrap(), &mut net),
            Err(ErrorCode::InvalidArgument)
        ));
        assert!(sock.needs_implicit_bind());
        sock.bind(local.parse().unwrap(), &mut net).unwrap();
        sock.finish_bind().unwrap();
        sock.drop(&mut net).unwrap();
    }
}

#[tokio::test]
async fn address_collision_preserves_unbound_socket() {
    let mut net = Network::default();
    let addr = "127.0.0.1:1234".parse().unwrap();
    let mut first = socket(SocketAddressFamily::Ipv4).await;
    let mut second = socket(SocketAddressFamily::Ipv4).await;
    first.bind(addr, &mut net).unwrap();
    assert!(matches!(
        second.bind(addr, &mut net),
        Err(ErrorCode::AddressInUse)
    ));
    assert!(second.needs_implicit_bind());
    first.drop(&mut net).unwrap();
    second.bind(addr, &mut net).unwrap();
    second.finish_bind().unwrap();
    second.drop(&mut net).unwrap();
}

#[tokio::test]
async fn connect_reconnect_disconnect_update_both_halves() {
    for (family, local, unspecified) in FAMILIES {
        for address in [local, unspecified] {
            let mut net = Network::default();
            let mut sock = socket(family).await;
            let mut peer: SocketAddr = local.parse().unwrap();
            peer.set_port(1234);
            assert!(matches!(
                sock.connect(peer, Plane::Virtual, None, &mut net),
                Err(ErrorCode::InvalidState)
            ));
            sock.bind(address.parse().unwrap(), &mut net).unwrap();
            assert!(matches!(
                sock.connect(peer, Plane::Virtual, None, &mut net),
                Err(ErrorCode::InvalidState)
            ));
            sock.finish_bind().unwrap();
            let bound = sock.local_address().unwrap();
            for port in [1234, 5678] {
                peer.set_port(port);
                sock.connect(peer, Plane::Virtual, None, &mut net).unwrap();
                assert!(sock.is_connected());
                assert!(sock.is_bound());
                assert_eq!(sock.remote_address().unwrap(), peer);
                let lo = match &sock {
                    UdpSocket::Loopback(lo) | UdpSocket::Unspecified { lo, .. } => lo,
                    _ => unreachable!(),
                };
                let registered = lo.local_address().unwrap();
                assert!(net.connect_udp(&peer, &registered).unwrap().is_some());
                let mut stranger = peer;
                stranger.set_port(9999);
                assert!(net.connect_udp(&stranger, &registered).unwrap().is_none());
            }
            sock.disconnect(&mut net).unwrap();
            assert!(sock.is_bound());
            assert!(!sock.is_connected());
            assert!(matches!(
                sock.remote_address(),
                Err(ErrorCode::InvalidState)
            ));
            assert_eq!(sock.local_address().unwrap().port(), bound.port());
            assert!(matches!(
                sock.disconnect(&mut net),
                Err(ErrorCode::InvalidState)
            ));
            sock.drop(&mut net).unwrap();
        }
    }
}

#[tokio::test]
async fn rejected_reconnect_preserves_peer_plane_and_deadline() {
    let mut sock = socket(SocketAddressFamily::Ipv4).await;
    let mut net = Network::default();
    sock.bind("0.0.0.0:0".parse().unwrap(), &mut net).unwrap();
    sock.finish_bind().unwrap();
    let peer = "127.0.0.1:1234".parse().unwrap();
    let deadline = std::time::Instant::now();
    sock.connect(peer, Plane::Host, Some(deadline), &mut net)
        .unwrap();
    for invalid in ["[::1]:1234", "127.0.0.1:0", "0.0.0.0:1234"] {
        assert!(matches!(
            sock.connect(invalid.parse().unwrap(), Plane::Virtual, None, &mut net),
            Err(ErrorCode::InvalidArgument)
        ));
        assert_eq!(sock.remote_address().unwrap(), peer);
        assert_eq!(sock.connected_plane(), Some(Plane::Host));
        assert_eq!(sock.connected_until_mut().map(|v| *v), Some(Some(deadline)));
    }
    sock.drop(&mut net).unwrap();
}

#[tokio::test]
async fn options_survive_bind_and_validate_boundaries() {
    for (family, local, unspecified) in FAMILIES {
        for address in [local, unspecified] {
            let mut net = Network::default();
            let mut sock = socket(family).await;
            sock.set_unicast_hop_limit(42).unwrap();
            sock.set_receive_buffer_size(8192).unwrap();
            sock.set_send_buffer_size(8192).unwrap();
            let receive = sock.receive_buffer_size().unwrap();
            let send = sock.send_buffer_size().unwrap();
            sock.bind(address.parse().unwrap(), &mut net).unwrap();
            sock.finish_bind().unwrap();
            assert_eq!(sock.unicast_hop_limit().unwrap(), 42);
            assert_eq!(sock.receive_buffer_size().unwrap(), receive);
            assert_eq!(sock.send_buffer_size().unwrap(), send);
            for hop in [1, u8::MAX] {
                sock.set_unicast_hop_limit(hop).unwrap();
                assert_eq!(sock.unicast_hop_limit().unwrap(), hop);
            }
            assert!(matches!(
                sock.set_unicast_hop_limit(0),
                Err(ErrorCode::InvalidArgument)
            ));
            assert_eq!(sock.unicast_hop_limit().unwrap(), u8::MAX);
            assert!(matches!(
                sock.set_receive_buffer_size(0),
                Err(ErrorCode::InvalidArgument)
            ));
            assert!(matches!(
                sock.set_send_buffer_size(0),
                Err(ErrorCode::InvalidArgument)
            ));
            sock.set_receive_buffer_size(4096).unwrap();
            sock.set_send_buffer_size(4096).unwrap();
            assert!(sock.receive_buffer_size().unwrap() >= 4096);
            assert!(sock.send_buffer_size().unwrap() >= 4096);
            if let UdpSocket::Unspecified { net, lo } = &sock {
                assert_eq!(
                    net.unicast_hop_limit().unwrap(),
                    lo.unicast_hop_limit().unwrap()
                );
                assert_eq!(lo.receive_buffer_size().unwrap(), 4096);
                assert_eq!(lo.send_buffer_size().unwrap(), 4096);
            }
            sock.drop(&mut net).unwrap();
        }
    }
}

#[tokio::test]
async fn loopback_send_buffer_resize_accounts_for_outstanding_permits() {
    let mut net = Network::default();
    let mut sock = socket(SocketAddressFamily::Ipv4).await;
    sock.bind("127.0.0.1:0".parse().unwrap(), &mut net).unwrap();
    sock.finish_bind().unwrap();
    sock.set_send_buffer_size(8).unwrap();
    let UdpSocket::Loopback(lo) = &mut sock else {
        unreachable!()
    };
    let crate::sockets::loopback::UdpState::Bound { permits, .. } = &lo.state else {
        unreachable!()
    };
    let permits = Arc::clone(permits);
    let held = permits.clone().try_acquire_many_owned(6).unwrap();
    lo.set_send_buffer_size(2).unwrap();
    assert_eq!(lo.send_buffer_size().unwrap(), 6);
    assert_eq!(permits.available_permits(), 0);
    drop(held);
    lo.set_send_buffer_size(2).unwrap();
    assert_eq!(permits.available_permits(), 2);
    lo.set_send_buffer_size(10).unwrap();
    assert_eq!(permits.available_permits(), 10);
    lo.set_send_buffer_size(u64::MAX).unwrap();
    assert_eq!(
        lo.send_buffer_size().unwrap(),
        u64::from(crate::sockets::loopback::UdpSocket::MAX_SEND_BUFFER_SIZE)
    );
    sock.drop(&mut net).unwrap();
}

#[tokio::test]
async fn address_check_is_copied_to_virtual_halves_and_streams() {
    let mut net = Network::default();
    for (family, local, unspecified) in FAMILIES {
        for address in [local, unspecified] {
            let mut sock = socket(family).await;
            let check = SocketAddrCheck::new(|_, _| {
                AddrDecision::Deny(crate::sockets::DenyReason::NotPermitted)
            });
            sock.set_socket_addr_check(Some(check));
            sock.bind(address.parse().unwrap(), &mut net).unwrap();
            sock.finish_bind().unwrap();
            let lo = match &mut sock {
                UdpSocket::Loopback(lo) | UdpSocket::Unspecified { lo, .. } => lo,
                _ => unreachable!(),
            };
            let (rx, tx) = lo.p2_udp_streams(None).unwrap();
            assert!(rx.socket_addr_check.is_some());
            assert!(tx.socket_addr_check.is_some());
            sock.set_socket_addr_check(None);
            assert!(sock.socket_addr_check().is_none());
            if let UdpSocket::Unspecified { lo, .. } = &sock {
                assert!(lo.socket_addr_check().is_none());
            }
            sock.drop(&mut net).unwrap();
        }
    }
}

#[tokio::test]
async fn quota_slot_follows_real_socket_lifetime() {
    use crate::host::quota::{GuestConnectionQuota, QuotaLimits};
    for address in [None, Some("0.0.0.0:0"), Some("127.0.0.1:0")] {
        let quota = GuestConnectionQuota::new(
            QuotaLimits {
                outbound_sockets: 1,
                ..Default::default()
            },
            None,
        );
        let mut sock = socket(SocketAddressFamily::Ipv4).await;
        let mut net = Network::default();
        sock.hold_quota_slot(quota.try_acquire_outbound_socket());
        assert!(quota.try_acquire_outbound_socket().is_none());
        if let Some(address) = address {
            sock.bind(address.parse().unwrap(), &mut net).unwrap();
            sock.finish_bind().unwrap();
        }
        if matches!(sock, UdpSocket::Loopback(_)) {
            assert!(quota.try_acquire_outbound_socket().is_some());
        } else {
            assert!(quota.try_acquire_outbound_socket().is_none());
        }
        sock.drop(&mut net).unwrap();
        assert!(quota.try_acquire_outbound_socket().is_some());
    }
}
