# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2024-01-XX

### Added

- Initial implementation of WebSocket client provider
- Support for WebSocket connections using tokio-tungstenite
- Implementation of `wasmcloud:websocket/client` interface
- Support for secure WebSocket connections (wss://)
- WebSocket subprotocol negotiation
- Custom header support for handshake
- Connection timeout configuration
- Automatic ping/pong keep-alive mechanism
- Comprehensive error handling
- Connection state management
- Message sending and receiving (text, binary, ping, pong, close)
- Proper connection lifecycle management
- Async/await support throughout
- Logging and instrumentation with tracing
- Unit tests for core functionality
- Documentation and examples

### Security

- TLS support for secure WebSocket connections
- Input validation for URLs and configuration parameters
- Safe handling of WebSocket frames 