use wasi::sockets::{
    instance_network::instance_network,
    network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress},
    udp::OutgoingDatagram,
    udp_create_socket::create_udp_socket,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let peer = std::env::var("WASH_UDP_PROGRESS_PEER")?.parse::<std::net::SocketAddrV4>()?;
    let octets = peer.ip().octets();
    let remote = IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port: peer.port(),
        address: (octets[0], octets[1], octets[2], octets[3]),
    });
    let network = instance_network();
    let socket = create_udp_socket(IpAddressFamily::Ipv4)?;
    socket.start_bind(
        &network,
        IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: 0,
            address: (0, 0, 0, 0),
        }),
    )?;
    socket.subscribe().block();
    socket.finish_bind()?;
    let (incoming, outgoing) = socket.stream(Some(remote))?;
    assert!(incoming.receive(1)?.is_empty());
    for seq in 1..=3 {
        let payload = format!("udp-progress-{seq}");
        if outgoing.check_send()? == 0 {
            outgoing.subscribe().block();
            assert!(outgoing.check_send()? > 0);
        }
        assert_eq!(
            outgoing.send(&[OutgoingDatagram {
                data: payload.as_bytes().to_vec(),
                remote_address: None
            }])?,
            1
        );
        let data = loop {
            let mut data = incoming.receive(1)?;
            if let Some(data) = data.pop() {
                break data;
            }
            incoming.subscribe().block();
        };
        assert_eq!(data.data, payload.as_bytes());
        assert!(incoming.receive(1)?.is_empty());
    }
    outgoing.check_send()?;
    outgoing.send(&[OutgoingDatagram {
        data: b"udp-progress-complete".to_vec(),
        remote_address: None,
    }])?;
    Ok(())
}
