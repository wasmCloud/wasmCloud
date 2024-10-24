// Code generated by wit-bindgen-go. DO NOT EDIT.

// Package udp represents the imported interface "wasi:sockets/udp@0.2.0".
package udp

import (
	"github.com/bytecodealliance/wasm-tools-go/cm"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/io/poll"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/sockets/network"
)

// IncomingDatagram represents the record "wasi:sockets/udp@0.2.0#incoming-datagram".
//
//	record incoming-datagram {
//		data: list<u8>,
//		remote-address: ip-socket-address,
//	}
type IncomingDatagram struct {
	_             cm.HostLayout
	Data          cm.List[uint8]
	RemoteAddress network.IPSocketAddress
}

// OutgoingDatagram represents the record "wasi:sockets/udp@0.2.0#outgoing-datagram".
//
//	record outgoing-datagram {
//		data: list<u8>,
//		remote-address: option<ip-socket-address>,
//	}
type OutgoingDatagram struct {
	_             cm.HostLayout
	Data          cm.List[uint8]
	RemoteAddress cm.Option[network.IPSocketAddress]
}

// UDPSocket represents the imported resource "wasi:sockets/udp@0.2.0#udp-socket".
//
//	resource udp-socket
type UDPSocket cm.Resource

// ResourceDrop represents the imported resource-drop for resource "udp-socket".
//
// Drops a resource handle.
//
//go:nosplit
func (self UDPSocket) ResourceDrop() {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketResourceDrop((uint32)(self0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [resource-drop]udp-socket
//go:noescape
func wasmimport_UDPSocketResourceDrop(self0 uint32)

// AddressFamily represents the imported method "address-family".
//
//	address-family: func() -> ip-address-family
//
//go:nosplit
func (self UDPSocket) AddressFamily() (result network.IPAddressFamily) {
	self0 := cm.Reinterpret[uint32](self)
	result0 := wasmimport_UDPSocketAddressFamily((uint32)(self0))
	result = (network.IPAddressFamily)((uint32)(result0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.address-family
//go:noescape
func wasmimport_UDPSocketAddressFamily(self0 uint32) (result0 uint32)

// FinishBind represents the imported method "finish-bind".
//
//	finish-bind: func() -> result<_, error-code>
//
//go:nosplit
func (self UDPSocket) FinishBind() (result cm.Result[network.ErrorCode, struct{}, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketFinishBind((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.finish-bind
//go:noescape
func wasmimport_UDPSocketFinishBind(self0 uint32, result *cm.Result[network.ErrorCode, struct{}, network.ErrorCode])

// LocalAddress represents the imported method "local-address".
//
//	local-address: func() -> result<ip-socket-address, error-code>
//
//go:nosplit
func (self UDPSocket) LocalAddress() (result cm.Result[IPSocketAddressShape, network.IPSocketAddress, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketLocalAddress((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.local-address
//go:noescape
func wasmimport_UDPSocketLocalAddress(self0 uint32, result *cm.Result[IPSocketAddressShape, network.IPSocketAddress, network.ErrorCode])

// ReceiveBufferSize represents the imported method "receive-buffer-size".
//
//	receive-buffer-size: func() -> result<u64, error-code>
//
//go:nosplit
func (self UDPSocket) ReceiveBufferSize() (result cm.Result[uint64, uint64, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketReceiveBufferSize((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.receive-buffer-size
//go:noescape
func wasmimport_UDPSocketReceiveBufferSize(self0 uint32, result *cm.Result[uint64, uint64, network.ErrorCode])

// RemoteAddress represents the imported method "remote-address".
//
//	remote-address: func() -> result<ip-socket-address, error-code>
//
//go:nosplit
func (self UDPSocket) RemoteAddress() (result cm.Result[IPSocketAddressShape, network.IPSocketAddress, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketRemoteAddress((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.remote-address
//go:noescape
func wasmimport_UDPSocketRemoteAddress(self0 uint32, result *cm.Result[IPSocketAddressShape, network.IPSocketAddress, network.ErrorCode])

// SendBufferSize represents the imported method "send-buffer-size".
//
//	send-buffer-size: func() -> result<u64, error-code>
//
//go:nosplit
func (self UDPSocket) SendBufferSize() (result cm.Result[uint64, uint64, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketSendBufferSize((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.send-buffer-size
//go:noescape
func wasmimport_UDPSocketSendBufferSize(self0 uint32, result *cm.Result[uint64, uint64, network.ErrorCode])

// SetReceiveBufferSize represents the imported method "set-receive-buffer-size".
//
//	set-receive-buffer-size: func(value: u64) -> result<_, error-code>
//
//go:nosplit
func (self UDPSocket) SetReceiveBufferSize(value uint64) (result cm.Result[network.ErrorCode, struct{}, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	value0 := (uint64)(value)
	wasmimport_UDPSocketSetReceiveBufferSize((uint32)(self0), (uint64)(value0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.set-receive-buffer-size
//go:noescape
func wasmimport_UDPSocketSetReceiveBufferSize(self0 uint32, value0 uint64, result *cm.Result[network.ErrorCode, struct{}, network.ErrorCode])

// SetSendBufferSize represents the imported method "set-send-buffer-size".
//
//	set-send-buffer-size: func(value: u64) -> result<_, error-code>
//
//go:nosplit
func (self UDPSocket) SetSendBufferSize(value uint64) (result cm.Result[network.ErrorCode, struct{}, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	value0 := (uint64)(value)
	wasmimport_UDPSocketSetSendBufferSize((uint32)(self0), (uint64)(value0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.set-send-buffer-size
//go:noescape
func wasmimport_UDPSocketSetSendBufferSize(self0 uint32, value0 uint64, result *cm.Result[network.ErrorCode, struct{}, network.ErrorCode])

// SetUnicastHopLimit represents the imported method "set-unicast-hop-limit".
//
//	set-unicast-hop-limit: func(value: u8) -> result<_, error-code>
//
//go:nosplit
func (self UDPSocket) SetUnicastHopLimit(value uint8) (result cm.Result[network.ErrorCode, struct{}, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	value0 := (uint32)(value)
	wasmimport_UDPSocketSetUnicastHopLimit((uint32)(self0), (uint32)(value0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.set-unicast-hop-limit
//go:noescape
func wasmimport_UDPSocketSetUnicastHopLimit(self0 uint32, value0 uint32, result *cm.Result[network.ErrorCode, struct{}, network.ErrorCode])

// StartBind represents the imported method "start-bind".
//
//	start-bind: func(network: borrow<network>, local-address: ip-socket-address) ->
//	result<_, error-code>
//
//go:nosplit
func (self UDPSocket) StartBind(network_ network.Network, localAddress network.IPSocketAddress) (result cm.Result[network.ErrorCode, struct{}, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	network0 := cm.Reinterpret[uint32](network_)
	localAddress0, localAddress1, localAddress2, localAddress3, localAddress4, localAddress5, localAddress6, localAddress7, localAddress8, localAddress9, localAddress10, localAddress11 := lower_IPSocketAddress(localAddress)
	wasmimport_UDPSocketStartBind((uint32)(self0), (uint32)(network0), (uint32)(localAddress0), (uint32)(localAddress1), (uint32)(localAddress2), (uint32)(localAddress3), (uint32)(localAddress4), (uint32)(localAddress5), (uint32)(localAddress6), (uint32)(localAddress7), (uint32)(localAddress8), (uint32)(localAddress9), (uint32)(localAddress10), (uint32)(localAddress11), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.start-bind
//go:noescape
func wasmimport_UDPSocketStartBind(self0 uint32, network0 uint32, localAddress0 uint32, localAddress1 uint32, localAddress2 uint32, localAddress3 uint32, localAddress4 uint32, localAddress5 uint32, localAddress6 uint32, localAddress7 uint32, localAddress8 uint32, localAddress9 uint32, localAddress10 uint32, localAddress11 uint32, result *cm.Result[network.ErrorCode, struct{}, network.ErrorCode])

// Stream represents the imported method "stream".
//
//	%stream: func(remote-address: option<ip-socket-address>) -> result<tuple<incoming-datagram-stream,
//	outgoing-datagram-stream>, error-code>
//
//go:nosplit
func (self UDPSocket) Stream(remoteAddress cm.Option[network.IPSocketAddress]) (result cm.Result[TupleIncomingDatagramStreamOutgoingDatagramStreamShape, cm.Tuple[IncomingDatagramStream, OutgoingDatagramStream], network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	remoteAddress0, remoteAddress1, remoteAddress2, remoteAddress3, remoteAddress4, remoteAddress5, remoteAddress6, remoteAddress7, remoteAddress8, remoteAddress9, remoteAddress10, remoteAddress11, remoteAddress12 := lower_OptionIPSocketAddress(remoteAddress)
	wasmimport_UDPSocketStream((uint32)(self0), (uint32)(remoteAddress0), (uint32)(remoteAddress1), (uint32)(remoteAddress2), (uint32)(remoteAddress3), (uint32)(remoteAddress4), (uint32)(remoteAddress5), (uint32)(remoteAddress6), (uint32)(remoteAddress7), (uint32)(remoteAddress8), (uint32)(remoteAddress9), (uint32)(remoteAddress10), (uint32)(remoteAddress11), (uint32)(remoteAddress12), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.stream
//go:noescape
func wasmimport_UDPSocketStream(self0 uint32, remoteAddress0 uint32, remoteAddress1 uint32, remoteAddress2 uint32, remoteAddress3 uint32, remoteAddress4 uint32, remoteAddress5 uint32, remoteAddress6 uint32, remoteAddress7 uint32, remoteAddress8 uint32, remoteAddress9 uint32, remoteAddress10 uint32, remoteAddress11 uint32, remoteAddress12 uint32, result *cm.Result[TupleIncomingDatagramStreamOutgoingDatagramStreamShape, cm.Tuple[IncomingDatagramStream, OutgoingDatagramStream], network.ErrorCode])

// Subscribe represents the imported method "subscribe".
//
//	subscribe: func() -> pollable
//
//go:nosplit
func (self UDPSocket) Subscribe() (result poll.Pollable) {
	self0 := cm.Reinterpret[uint32](self)
	result0 := wasmimport_UDPSocketSubscribe((uint32)(self0))
	result = cm.Reinterpret[poll.Pollable]((uint32)(result0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.subscribe
//go:noescape
func wasmimport_UDPSocketSubscribe(self0 uint32) (result0 uint32)

// UnicastHopLimit represents the imported method "unicast-hop-limit".
//
//	unicast-hop-limit: func() -> result<u8, error-code>
//
//go:nosplit
func (self UDPSocket) UnicastHopLimit() (result cm.Result[uint8, uint8, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_UDPSocketUnicastHopLimit((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]udp-socket.unicast-hop-limit
//go:noescape
func wasmimport_UDPSocketUnicastHopLimit(self0 uint32, result *cm.Result[uint8, uint8, network.ErrorCode])

// IncomingDatagramStream represents the imported resource "wasi:sockets/udp@0.2.0#incoming-datagram-stream".
//
//	resource incoming-datagram-stream
type IncomingDatagramStream cm.Resource

// ResourceDrop represents the imported resource-drop for resource "incoming-datagram-stream".
//
// Drops a resource handle.
//
//go:nosplit
func (self IncomingDatagramStream) ResourceDrop() {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_IncomingDatagramStreamResourceDrop((uint32)(self0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [resource-drop]incoming-datagram-stream
//go:noescape
func wasmimport_IncomingDatagramStreamResourceDrop(self0 uint32)

// Receive represents the imported method "receive".
//
//	receive: func(max-results: u64) -> result<list<incoming-datagram>, error-code>
//
//go:nosplit
func (self IncomingDatagramStream) Receive(maxResults uint64) (result cm.Result[cm.List[IncomingDatagram], cm.List[IncomingDatagram], network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	maxResults0 := (uint64)(maxResults)
	wasmimport_IncomingDatagramStreamReceive((uint32)(self0), (uint64)(maxResults0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]incoming-datagram-stream.receive
//go:noescape
func wasmimport_IncomingDatagramStreamReceive(self0 uint32, maxResults0 uint64, result *cm.Result[cm.List[IncomingDatagram], cm.List[IncomingDatagram], network.ErrorCode])

// Subscribe represents the imported method "subscribe".
//
//	subscribe: func() -> pollable
//
//go:nosplit
func (self IncomingDatagramStream) Subscribe() (result poll.Pollable) {
	self0 := cm.Reinterpret[uint32](self)
	result0 := wasmimport_IncomingDatagramStreamSubscribe((uint32)(self0))
	result = cm.Reinterpret[poll.Pollable]((uint32)(result0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]incoming-datagram-stream.subscribe
//go:noescape
func wasmimport_IncomingDatagramStreamSubscribe(self0 uint32) (result0 uint32)

// OutgoingDatagramStream represents the imported resource "wasi:sockets/udp@0.2.0#outgoing-datagram-stream".
//
//	resource outgoing-datagram-stream
type OutgoingDatagramStream cm.Resource

// ResourceDrop represents the imported resource-drop for resource "outgoing-datagram-stream".
//
// Drops a resource handle.
//
//go:nosplit
func (self OutgoingDatagramStream) ResourceDrop() {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_OutgoingDatagramStreamResourceDrop((uint32)(self0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [resource-drop]outgoing-datagram-stream
//go:noescape
func wasmimport_OutgoingDatagramStreamResourceDrop(self0 uint32)

// CheckSend represents the imported method "check-send".
//
//	check-send: func() -> result<u64, error-code>
//
//go:nosplit
func (self OutgoingDatagramStream) CheckSend() (result cm.Result[uint64, uint64, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	wasmimport_OutgoingDatagramStreamCheckSend((uint32)(self0), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]outgoing-datagram-stream.check-send
//go:noescape
func wasmimport_OutgoingDatagramStreamCheckSend(self0 uint32, result *cm.Result[uint64, uint64, network.ErrorCode])

// Send represents the imported method "send".
//
//	send: func(datagrams: list<outgoing-datagram>) -> result<u64, error-code>
//
//go:nosplit
func (self OutgoingDatagramStream) Send(datagrams cm.List[OutgoingDatagram]) (result cm.Result[uint64, uint64, network.ErrorCode]) {
	self0 := cm.Reinterpret[uint32](self)
	datagrams0, datagrams1 := cm.LowerList(datagrams)
	wasmimport_OutgoingDatagramStreamSend((uint32)(self0), (*OutgoingDatagram)(datagrams0), (uint32)(datagrams1), &result)
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]outgoing-datagram-stream.send
//go:noescape
func wasmimport_OutgoingDatagramStreamSend(self0 uint32, datagrams0 *OutgoingDatagram, datagrams1 uint32, result *cm.Result[uint64, uint64, network.ErrorCode])

// Subscribe represents the imported method "subscribe".
//
//	subscribe: func() -> pollable
//
//go:nosplit
func (self OutgoingDatagramStream) Subscribe() (result poll.Pollable) {
	self0 := cm.Reinterpret[uint32](self)
	result0 := wasmimport_OutgoingDatagramStreamSubscribe((uint32)(self0))
	result = cm.Reinterpret[poll.Pollable]((uint32)(result0))
	return
}

//go:wasmimport wasi:sockets/udp@0.2.0 [method]outgoing-datagram-stream.subscribe
//go:noescape
func wasmimport_OutgoingDatagramStreamSubscribe(self0 uint32) (result0 uint32)
