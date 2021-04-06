[![crates.io](https://img.shields.io/crates/v/wasmcloud-telnet.svg)](https://crates.io/crates/wasmcloud-telnet)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/TELNET/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wasmcloud-telnet.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-telnet/badge.svg)](https://docs.rs/wasmcloud-telnet)

# wasmCloud Telnet Capability Provider

The telnet capability provider will start a new telnet (socket) server for each actor that binds to it, using the following configuration variables from the binding:

* `PORT` - the port number on which to start the server
* `MOTD` - A file name containing the "message of the day" or login banner for the telnet server
