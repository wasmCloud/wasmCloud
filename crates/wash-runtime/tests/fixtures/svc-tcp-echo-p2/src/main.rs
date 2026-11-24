//! The p2 twin of `svc-tcp-echo`: echoes on the virtual loopback 127.0.0.1:9099.
//! A `trap` request traps the instance, so a test can restart the service.
use wasi::sockets::{
    instance_network::instance_network,
    network::{ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress},
    tcp_create_socket::create_tcp_socket,
};

const ECHO_PORT: u16 = 9099;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = create_tcp_socket(IpAddressFamily::Ipv4)?;
    listener.start_bind(
        &instance_network(),
        IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port: ECHO_PORT,
            address: (127, 0, 0, 1),
        }),
    )?;
    listener.subscribe().block();
    listener.finish_bind()?;
    listener.start_listen()?;
    listener.subscribe().block();
    listener.finish_listen()?;
    loop {
        listener.subscribe().block();
        let (_socket, input, output) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(ErrorCode::WouldBlock) => continue,
            Err(error) => return Err(error.into()),
        };
        let request = input.blocking_read(1024)?;
        if request == b"trap" {
            panic!("svc-tcp-echo-p2: deliberate trap for the restart test");
        }
        let mut reply = b"echo:".to_vec();
        reply.extend_from_slice(&request);
        output.blocking_write_and_flush(&reply)?;
    }
}
