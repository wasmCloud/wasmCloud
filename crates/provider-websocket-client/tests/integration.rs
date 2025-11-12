use wasmcloud_provider_websocket_client::WebSocketClientProvider;

#[tokio::test]
async fn test_provider_creation() {
    let provider = WebSocketClientProvider::default();
    assert_eq!(WebSocketClientProvider::name(), "websocket-client-provider");
}

#[tokio::test]
async fn test_provider_lifecycle() {
    let provider = WebSocketClientProvider::new();
    
    // Test shutdown
    provider.shutdown().await.expect("Provider shutdown should succeed");
} 