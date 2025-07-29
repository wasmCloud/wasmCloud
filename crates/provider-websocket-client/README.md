# WebSocket Client Provider

A wasmCloud capability provider that implements the `wasmcloud:websocket/client` interface, enabling components to connect to WebSocket servers and exchange messages in real-time.

## Features

- **Full WebSocket Support**: Complete implementation of the WebSocket protocol including text, binary, ping, pong, and close frames
- **Secure Connections**: Support for both `ws://` and `wss://` (WebSocket Secure) connections
- **Subprotocol Negotiation**: Support for WebSocket subprotocol selection
- **Custom Headers**: Ability to send custom headers during the WebSocket handshake
- **Connection Management**: Automatic connection lifecycle management with proper cleanup
- **Keep-alive Support**: Configurable ping/pong keep-alive mechanism
- **Timeout Control**: Configurable connection and operation timeouts
- **Error Handling**: Comprehensive error handling with detailed error messages

## Configuration

The provider supports the following configuration options through the WebSocket client config:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | required | WebSocket URL (ws:// or wss://) |
| `subprotocols` | list<string> | none | Optional subprotocols to request |
| `headers` | list<header> | none | Custom headers for handshake |
| `timeout_ms` | u32 | 30000 | Connection timeout in milliseconds |
| `max_message_size` | u32 | none | Maximum message size in bytes |
| `enable_keepalive` | bool | false | Enable automatic ping/pong keep-alive |
| `keepalive_interval_ms` | u32 | 30000 | Keep-alive interval in milliseconds |

## Usage

### Component Example

```rust
use wasmcloud::websocket::client::{connect, ClientConfig};
use wasmcloud::websocket::types::Message;

// Create WebSocket configuration
let config = ClientConfig {
    url: "wss://echo.websocket.org".to_string(),
    subprotocols: Some(vec!["chat".to_string()]),
    headers: None,
    timeout_ms: Some(5000),
    max_message_size: Some(1024 * 1024), // 1MB
    enable_keepalive: Some(true),
    keepalive_interval_ms: Some(30000),
};

// Connect to WebSocket server
let connection = connect(config)?;

// Send a message
let message = Message::Text("Hello, WebSocket!".to_string());
connection.send(message)?;

// Receive messages
if let Some(message) = connection.receive()? {
    match message {
        Message::Text(text) => println!("Received: {}", text),
        Message::Binary(data) => println!("Received binary data: {} bytes", data.len()),
        Message::Close(info) => println!("Connection closed: {:?}", info),
        _ => {}
    }
}

// Close connection
connection.close(Some(1000), Some("Normal closure".to_string()))?;
```

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test
```

## Dependencies

- `tokio-tungstenite` - WebSocket client implementation
- `tokio` - Async runtime
- `wasmcloud-provider-sdk` - wasmCloud provider SDK
- `url` - URL parsing
- `anyhow` - Error handling
- `tracing` - Logging and instrumentation

## License

Apache 2.0 