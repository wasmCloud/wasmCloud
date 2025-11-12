//! WebSocket client capability provider for wasmCloud
//! 
//! This provider implements the wasmcloud:websocket/client interface,
//! allowing components to connect to WebSocket servers and exchange messages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use serde::{Serialize, Deserialize};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async, tungstenite::Message as TungsteniteMessage, 
    tungstenite::protocol::CloseFrame, WebSocketStream, MaybeTlsStream
};
use tracing::{debug, error, instrument};
use url::Url;

use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, run_provider, serve_provider_exports,
    Context, LinkConfig, LinkDeleteInfo, Provider, propagate_trace_for_ctx,
};
use ::wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn, ResourceBorrowDecoder};
use wit_bindgen_wrpc::bytes::Bytes;


mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:websocket/client@0.1.1-draft": generate,
            "wasmcloud:websocket/types@0.1.1-draft": generate,
        },
    });
}

use bindings::exports::wasmcloud::websocket as imported_websocket;

// Type aliases for clarity
type WebSocketConnection = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// WebSocket connection state management
#[derive(Debug)]
struct ConnectionState {
    /// Current connection state
    state: imported_websocket::types::ConnectionState,
    /// URL of the connection
    url: String,
    /// Negotiated subprotocol
    subprotocol: Option<String>,
    /// Message receiver for incoming messages
    message_receiver: Option<mpsc::UnboundedReceiver<imported_websocket::types::Message>>,
    /// Message sender for outgoing messages  
    message_sender: Option<mpsc::UnboundedSender<TungsteniteMessage>>,
    /// Handle to background task managing the connection
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ConnectionState {
    fn new(url: String) -> Self {
        Self {
            state: imported_websocket::types::ConnectionState::Connecting,
            url,
            subprotocol: None,
            message_receiver: None,
            message_sender: None,
            task_handle: None,
        }
    }
}

/// WebSocket client provider implementation
#[derive(Clone, Default)]
pub struct WebSocketClientProvider {
    /// Store connections by component ID and connection ID
    connections: Arc<RwLock<HashMap<String, HashMap<u32, ConnectionState>>>>,
    /// Connection counter for generating unique IDs
    connection_counter: Arc<tokio::sync::Mutex<u32>>,
}

impl WebSocketClientProvider {
    /// Create a new provider instance
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn name() -> &'static str {
        "websocket-client-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let _host_data = load_host_data().context("failed to load host data")?;
        let flamegraph_path = std::env::var("PROVIDER_WEBSOCKET_CLIENT_FLAMEGRAPH_PATH").ok();
        initialize_observability!(Self::name(), flamegraph_path);
        
        let provider = WebSocketClientProvider::new();
        
        let shutdown = run_provider(provider.clone(), Self::name())
            .await
            .context("failed to run provider")?;
        
        let connection = get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
            
        // Use the provider for serving exports
        serve_provider_exports(&wrpc, provider, shutdown, bindings::serve)
            .await
            .context("failed to serve provider exports")
    }

    /// Get next connection ID
    async fn next_connection_id(&self) -> u32 {
        let mut counter = self.connection_counter.lock().await;
        *counter += 1;
        *counter
    }

    /// Create new WebSocket connection
    async fn create_connection(
        &self,
        component_id: &str,
        config: imported_websocket::client::ClientConfig,
    ) -> Result<u32, imported_websocket::types::WebsocketError> {
        let _url = Url::parse(&config.url)
            .map_err(|e| imported_websocket::types::WebsocketError::InvalidUrl(format!("Invalid URL: {}", e)))?;

        let connection_id = self.next_connection_id().await;
        
        // Create connection state
        let conn_state = ConnectionState::new(config.url.clone());
        
        // Store connection
        let mut connections = self.connections.write().await;
        connections.entry(component_id.to_string())
            .or_insert_with(HashMap::new)
            .insert(connection_id, conn_state);
        drop(connections);

        // Attempt to connect
        self.connect_websocket(component_id, connection_id, config).await?;
        
        Ok(connection_id)
    }

    /// Connect to WebSocket server
    async fn connect_websocket(
        &self,
        component_id: &str,
        connection_id: u32,
        config: imported_websocket::client::ClientConfig,
    ) -> Result<(), imported_websocket::types::WebsocketError> {
        let timeout_duration = config.timeout_ms
            .map(|ms| Duration::from_millis(ms as u64))
            .unwrap_or(Duration::from_secs(30));

        let connection_result = timeout(timeout_duration, connect_async(&config.url)).await
            .map_err(|_| imported_websocket::types::WebsocketError::Timeout)?
            .map_err(|e| imported_websocket::types::WebsocketError::ConnectionFailed(format!("Connection failed: {}", e)))?;

        let (ws_stream, response) = connection_result;
        
        // Get subprotocol from response headers
        let subprotocol = response.headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Create message channels for bidirectional communication
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel(); // For incoming messages
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel(); // For outgoing messages
        
        // Start background task to handle WebSocket messages
        let task_handle = self.start_connection_task(
            component_id.to_string(), 
            connection_id, 
            ws_stream, 
            incoming_tx,
            outgoing_rx
        ).await;

        // Update connection state
        let mut connections = self.connections.write().await;
        if let Some(component_connections) = connections.get_mut(component_id) {
            if let Some(conn_state) = component_connections.get_mut(&connection_id) {
                conn_state.state = imported_websocket::types::ConnectionState::Open;
                conn_state.subprotocol = subprotocol;
                conn_state.message_receiver = Some(incoming_rx);
                conn_state.message_sender = Some(outgoing_tx);
                conn_state.task_handle = Some(task_handle);
            }
        }

        Ok(())
    }

    /// Start background task to handle WebSocket connection
    async fn start_connection_task(
        &self,
        component_id: String,
        connection_id: u32,
        ws_stream: WebSocketConnection,
        incoming_sender: mpsc::UnboundedSender<imported_websocket::types::Message>,
        mut outgoing_receiver: mpsc::UnboundedReceiver<TungsteniteMessage>,
    ) -> tokio::task::JoinHandle<()> {
        let connections = Arc::clone(&self.connections);
        
        tokio::spawn(async move {
            let (mut ws_sender, mut ws_receiver) = ws_stream.split();
            
            loop {
                tokio::select! {
                    // Handle incoming messages from WebSocket
                    msg_result = ws_receiver.next() => {
                        match msg_result {
                            Some(Ok(msg)) => {
                                let typed_msg = match msg {
                                    TungsteniteMessage::Text(text) => {
                                        imported_websocket::types::Message::Text(text)
                                    }
                                    TungsteniteMessage::Binary(data) => {
                                        imported_websocket::types::Message::Binary(data.into())
                                    }
                                    TungsteniteMessage::Ping(data) => {
                                        imported_websocket::types::Message::Ping(data.into())
                                    }
                                    TungsteniteMessage::Pong(data) => {
                                        imported_websocket::types::Message::Pong(data.into())
                                    }
                                    TungsteniteMessage::Close(close_frame) => {
                                        let close_info = close_frame.map(|cf| imported_websocket::types::CloseInfo {
                                            code: cf.code.into(),
                                            reason: cf.reason.to_string().into(),
                                        }).unwrap_or(imported_websocket::types::CloseInfo {
                                            code: 1000,
                                            reason: None,
                                        });
                                        imported_websocket::types::Message::Close(close_info)
                                    }
                                    TungsteniteMessage::Frame(_) => continue, // Skip raw frames
                                };

                                if incoming_sender.send(typed_msg).is_err() {
                                    break; // Receiver dropped
                                }
                            }
                            Some(Err(e)) => {
                                error!("WebSocket receive error for component {}, connection {}: {}", component_id, connection_id, e);
                                break;
                            }
                            None => {
                                debug!("WebSocket stream ended for component {}, connection {}", component_id, connection_id);
                                break;
                            }
                        }
                    }
                    
                    // Handle outgoing messages to WebSocket
                    msg = outgoing_receiver.recv() => {
                        match msg {
                            Some(msg) => {
                                if let Err(e) = ws_sender.send(msg).await {
                                    error!("WebSocket send error for component {}, connection {}: {}", component_id, connection_id, e);
                                    break;
                                }
                            }
                            None => {
                                debug!("Outgoing message channel closed for component {}, connection {}", component_id, connection_id);
                                break;
                            }
                        }
                    }
                }
            }

            // Mark connection as closed
            let mut connections = connections.write().await;
            if let Some(component_connections) = connections.get_mut(&component_id) {
                if let Some(conn_state) = component_connections.get_mut(&connection_id) {
                    conn_state.state = imported_websocket::types::ConnectionState::Closed;
                }
            }
        })
    }
}

impl Provider for WebSocketClientProvider {
    /// Handle new component link
    // #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let source_id = link_config.source_id;
        debug!("WebSocket client provider linked to component {}", source_id);
        
        // Initialize connection storage for this component
        let mut connections = self.connections.write().await;
        connections.entry(source_id.to_string())
            .or_insert_with(HashMap::new);
        
        Ok(())
    }

    /// Handle component link deletion when we are the target
    // #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let source_id = info.get_source_id();
        tracing::Span::current().record("source_id", source_id);
        debug!("Deleting WebSocket client link for component {}", source_id);
        
        // Close all connections for this component
        let mut connections = self.connections.write().await;
        if let Some(mut component_connections) = connections.remove(source_id) {
            for (_, mut conn_state) in component_connections.drain() {
                conn_state.state = imported_websocket::types::ConnectionState::Closed;
                if let Some(handle) = conn_state.task_handle {
                    handle.abort();
                }
            }
        }
        
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug)]
// Simple connection resource that stores component ID and connection ID
pub struct Connection {
    pub component_id: String,
    pub connection_id: u32,
}

impl Connection {
    /// Parse connection from resource identifier "component_id:connection_id"
    fn from_identifier(identifier: &[u8]) -> anyhow::Result<Self> {
        let id_str = String::from_utf8(identifier.to_vec())?;
        let parts: Vec<&str> = id_str.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!("Invalid connection identifier format"));
        }
        
        let component_id = parts[0].to_string();
        let connection_id = parts[1].parse::<u32>()?;
        
        Ok(Connection {
            component_id,
            connection_id,
        })
    }

    fn get_identifier(&self) -> Vec<u8> {
        format!("{}:{}", self.component_id, self.connection_id).into_bytes()
    }
}

impl Into<Bytes> for Connection {
    fn into(self) -> Bytes {
        self.get_identifier().into()
    }
}

impl imported_websocket::client::HandlerConnection<Option<Context>> for WebSocketClientProvider {
    async fn send(
        &self,
        ctx: Option<Context>,
        connection: ResourceBorrow<imported_websocket::client::Connection>,
        message: imported_websocket::types::Message,
    ) -> anyhow::Result<Result<(), imported_websocket::types::WebsocketError>> {
      
        propagate_trace_for_ctx!(ctx);
        
        // Parse connection from resource identifier
        let connection = Connection::from_identifier(connection.as_ref())?;
        
        // Convert typed message to tungstenite format
        let tungstenite_msg = match message {
            imported_websocket::types::Message::Text(text) => TungsteniteMessage::Text(text),
            imported_websocket::types::Message::Binary(data) => TungsteniteMessage::Binary(data.to_vec()),
            imported_websocket::types::Message::Ping(data) => TungsteniteMessage::Ping(data.to_vec()),
            imported_websocket::types::Message::Pong(data) => TungsteniteMessage::Pong(data.to_vec()),
            imported_websocket::types::Message::Close(close_info) => {
                let close_frame = CloseFrame {
                    code: close_info.code.into(),
                    reason: close_info.reason.unwrap_or_default().into(),
                };
                TungsteniteMessage::Close(Some(close_frame))
            }
        };
        
        // Get connection state and send message
        let connections = self.connections.read().await;
        let component_connections = connections.get(&connection.component_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
        
        let conn_state = component_connections.get(&connection.connection_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
            
        if conn_state.state != imported_websocket::types::ConnectionState::Open {
            return Ok(Err(imported_websocket::types::WebsocketError::ConnectionClosed));
        }
        
        // Send message through the outgoing channel
        if let Some(ref sender) = conn_state.message_sender {
            sender.send(tungstenite_msg)
                .map_err(|_| imported_websocket::types::WebsocketError::ConnectionClosed)?;
            Ok(Ok(()))
        } else {
            Ok(Err(imported_websocket::types::WebsocketError::ConnectionClosed))
        }
    }

    async fn receive(
        &self,
        ctx: Option<Context>,
        connection: ResourceBorrow<imported_websocket::client::Connection>,
    ) -> anyhow::Result<Result<Option<imported_websocket::types::Message>, imported_websocket::types::WebsocketError>> {
       
        propagate_trace_for_ctx!(ctx);

        // Parse connection from resource identifier
        let connection = Connection::from_identifier(connection.as_ref())?;
        
        let mut connections = self.connections.write().await;
        let component_connections = connections.get_mut(&connection.component_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
        
        let conn_state = component_connections.get_mut(&connection.connection_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
            
        if let Some(ref mut receiver) = conn_state.message_receiver {
            Ok(Ok(receiver.try_recv().ok()))
        } else {
            Ok(Err(imported_websocket::types::WebsocketError::ConnectionClosed))
        }
    }

    async fn close(
        &self,
        ctx: Option<Context>,
        self_: ResourceBorrow<imported_websocket::client::Connection>,
        _code: Option<u16>,
        _reason: Option<String>,
    ) -> anyhow::Result<Result<(), imported_websocket::types::WebsocketError>> {
       
        propagate_trace_for_ctx!(ctx);
        // Parse connection from resource identifier
        let connection = Connection::from_identifier(self_.as_ref())?;
        
        let mut connections = self.connections.write().await;
        let component_connections = connections.get_mut(&connection.component_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
        
        let conn_state = component_connections.get_mut(&connection.connection_id)
            .ok_or_else(|| imported_websocket::types::WebsocketError::ConnectionClosed)?;
            
        conn_state.state = imported_websocket::types::ConnectionState::Closing;
        
        if let Some(handle) = conn_state.task_handle.take() {
            handle.abort();
        }
        
        conn_state.state = imported_websocket::types::ConnectionState::Closed;
        Ok(Ok(()))
    }

    async fn get_state(
        &self,
        ctx: Option<Context>,
        connection: ResourceBorrow<imported_websocket::client::Connection>,
    ) -> anyhow::Result<imported_websocket::types::ConnectionState> {
       
        propagate_trace_for_ctx!(ctx);
        // Parse connection from resource identifier
        let connection = Connection::from_identifier(connection.as_ref())?;
        
        let connections = self.connections.read().await;
        let component_connections = connections.get(&connection.component_id);
        
        if let Some(component_connections) = component_connections {
            if let Some(conn_state) = component_connections.get(&connection.connection_id) {
                return Ok(conn_state.state.clone());
            }
        }
        
        Ok(imported_websocket::types::ConnectionState::Closed)
    }

    async fn get_url(
        &self,
        ctx: Option<Context>,
        connection: ResourceBorrow<imported_websocket::client::Connection>,
    ) -> anyhow::Result<String> {
       
        propagate_trace_for_ctx!(ctx);
        // Parse connection from resource identifier
        let connection = Connection::from_identifier(connection.as_ref())?;
        
        let connections = self.connections.read().await;
        let component_connections = connections.get(&connection.component_id);
        
        if let Some(component_connections) = component_connections {
            if let Some(conn_state) = component_connections.get(&connection.connection_id) {
                return Ok(conn_state.url.clone());
            }
        }
        
        Ok(String::new())
    }

    async fn get_subprotocol(
        &self,
        ctx: Option<Context>,
        connection: ResourceBorrow<imported_websocket::client::Connection>,
    ) -> anyhow::Result<Option<String>> {
        propagate_trace_for_ctx!(ctx);

        let connection = Connection::from_identifier(connection.as_ref())?;
        
        let connections = self.connections.read().await;
        let component_connections = connections.get(&connection.component_id);
        
        if let Some(component_connections) = component_connections {
            if let Some(conn_state) = component_connections.get(&connection.connection_id) {
                return Ok(conn_state.subprotocol.clone());
            }
        }
        
        Ok(None)
    }
}

// Implement the WebSocket client interface
impl imported_websocket::client::Handler<Option<Context>> for WebSocketClientProvider {
    async fn connect(
        &self,
        context: Option<Context>,
        config: imported_websocket::client::ClientConfig,
    ) -> anyhow::Result<Result<ResourceOwn<imported_websocket::client::Connection>, imported_websocket::types::WebsocketError>> {
        
        propagate_trace_for_ctx!(context);
        
        // Get component ID from context
        let ctx = context.ok_or_else(|| {
            imported_websocket::types::WebsocketError::ConnectionFailed(
                "No component context available".to_string()
            )
        })?;
        
        let component_id = ctx.component.as_ref().ok_or_else(|| {
            imported_websocket::types::WebsocketError::ConnectionFailed(
                "No component ID in context".to_string()
            )
        })?;
        
        let connection_id = match self.create_connection(component_id, config).await {
            Ok(id) => id,
            Err(e) => return Ok(Err(e)),
        };

        let connection = Connection {
            component_id: component_id.to_string(),
            connection_id,
        };
        
        Ok(Ok(ResourceOwn::<imported_websocket::client::Connection>::new(connection)))
    }
}

