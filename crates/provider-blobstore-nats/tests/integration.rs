use anyhow::{Context as _, Result};
use bytes::Bytes;
use futures::{stream, StreamExt as _};
use std::{collections::HashMap, time::Duration};
use tokio::io::AsyncReadExt;
use wasmcloud_provider_blobstore_nats::NatsBlobstoreProvider;
use wasmcloud_provider_sdk::{
    get_connection, provider::initialize_host_data, run_provider, serve_provider_exports, HostData,
    InterfaceLinkDefinition,
};
use wasmcloud_test_util::testcontainers::{AsyncRunner as _, ContainerAsync, ImageExt, NatsServer};
use wrpc_interface_blobstore::bindings::{
    serve,
    wrpc::blobstore::{blobstore, types::ObjectId},
};

struct TestEnv {
    _nats: ContainerAsync<NatsServer>,
    nats_address: String,
    lattice: String,
    test_suite: String,
}

impl TestEnv {
    pub async fn new(lattice: &str, test_suite: &str) -> Result<Self> {
        // Get configurable startup timeout
        let timeout = std::env::var("TESTCONTAINERS_NATS_STARTUP_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);

        // Initialize NATS server with more stable configuration
        let nats = NatsServer::default()
            .with_startup_timeout(Duration::from_secs(timeout))
            .with_cmd([
                "--jetstream",
                "--store_dir",
                "/tmp/nats", // Persistent storage
            ])
            .start()
            .await
            .context("should start nats-server")?;

        let port = nats
            .get_host_port_ipv4(4222)
            .await
            .context("should get host port")?;
        let nats_address = format!("0.0.0.0:{}", port);

        // Longer initial wait for server stability
        tokio::time::sleep(Duration::from_secs(5)).await;

        let env = Self {
            _nats: nats,
            nats_address: nats_address.clone(),
            lattice: lattice.to_string(),
            test_suite: test_suite.to_string(),
        };

        // Initialize host data before validation
        let host_data = HostData {
            lattice_rpc_url: env.nats_address.clone(),
            lattice_rpc_prefix: lattice.to_string(),
            provider_key: test_suite.to_string(),
            config: HashMap::new(),
            link_definitions: vec![InterfaceLinkDefinition {
                source_id: "test-component".to_string(),
                target: test_suite.to_string(),
                name: "default".to_string(),
                wit_namespace: "wrpc".to_string(),
                wit_package: "blobstore".to_string(),
                interfaces: vec!["blobstore".to_string()],
                source_config: HashMap::new(),
                target_config: HashMap::from([
                    (
                        "CONFIG_NATS_URI".to_string(),
                        format!("nats://{}", nats_address),
                    ),
                    ("CONFIG_NATS_MAX_WRITE_WAIT".to_string(), "30".to_string()),
                ]),
                source_secrets: None,
                target_secrets: None,
            }],
            ..Default::default()
        };
        initialize_host_data(host_data).expect("should be able to initialize host data");

        // Validate connection after setup
        env.validate_nats_connection()
            .await
            .context("failed to validate NATS connection")?;

        Ok(env)
    }

    pub fn nats_endpoint(address: &str) -> String {
        format!("nats://{}", address)
    }

    async fn nats_client(&self) -> Result<async_nats::Client> {
        async_nats::ConnectOptions::new()
            .name("test-nats-client")
            .retry_on_initial_connect()
            .max_reconnects(5)
            .connect(Self::nats_endpoint(&self.nats_address))
            .await
            .map_err(anyhow::Error::msg)
    }

    pub async fn create_object_store(
        &self,
        nats_container: &str,
    ) -> Result<async_nats::jetstream::object_store::ObjectStore> {
        let client = self.nats_client().await?;
        let jetstream = async_nats::jetstream::new(client);

        // Safety: Delete any existing container first
        // This ensures we always start with a clean slate, and eliminates the need to do a pre-shutdown cleanup
        let _ = jetstream
            .delete_object_store(nats_container.to_string())
            .await;

        let object_store = jetstream
            .create_object_store(async_nats::jetstream::object_store::Config {
                bucket: nats_container.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(object_store)
    }

    pub async fn delete_object_store(&self, nats_container: &str) -> Result<()> {
        let client = self.nats_client().await?;
        let jetstream = async_nats::jetstream::new(client);
        jetstream
            .delete_object_store(nats_container.to_string())
            .await?;
        Ok(())
    }

    pub async fn object_store_exists(&self, bucket_name: &str) -> Result<bool> {
        let client = self.nats_client().await?;
        let jetstream = async_nats::jetstream::new(client);
        match jetstream.get_object_store(bucket_name.to_string()).await {
            Ok(_) => Ok(true),
            Err(e) => {
                if let async_nats::jetstream::context::ObjectStoreErrorKind::GetStore = e.kind() {
                    Ok(false)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    pub async fn start_provider(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        // TODO: we should not need the `cfg!(debug_assertions)` check here, but until
        // we resolve the `stack overflow` resulting from `tracing`` instrument calls
        // inside wrpc, `NatsBlobstoreProvider::run` can only be used to run the provider
        // when the `--release` is passed (to `cargo test`).
        let handle = if cfg!(debug_assertions) {
            let provider = NatsBlobstoreProvider::default();
            let shutdown = run_provider(provider.clone(), "blobstore-nats-provider")
                .await
                .context("should've been able to run provider")?;
            let connection = get_connection();
            let wrpc = connection
                .get_wrpc_client(connection.provider_key())
                .await?;
            tokio::spawn(async move {
                serve_provider_exports(&wrpc, provider, shutdown, serve)
                    .await
                    .context("failed to serve provider exports")
            })
        } else {
            tokio::spawn(async move {
                NatsBlobstoreProvider::run()
                    .await
                    .context("failed to run the provider")
            })
        };
        Ok(handle)
    }

    pub async fn wrpc_client(&self) -> Result<wrpc_transport_nats::Client> {
        let nats = self.nats_client().await?;
        let prefix = format!("{}.{}", self.lattice, self.test_suite);
        wrpc_transport_nats::Client::new(nats, prefix, None).await
    }

    pub fn wrpc_context(&self) -> Option<async_nats::HeaderMap> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("source-id", "test-component");
        headers.insert("link-name", "default");
        Some(headers)
    }

    async fn validate_nats_connection(&self) -> Result<()> {
        let client = self.nats_client().await?;

        // Try a simple ping-pong to verify connection
        client
            .publish("test.ping", "ping".into())
            .await
            .context("failed to publish test message")?;

        Ok(())
    }
}

/// Tests the creation of a new container in NATS Object Store
///
/// Flow:
/// 1. Verifies container doesn't exist initially
/// 2. Creates a new container, using the provider's `create-container` API
/// 3. Verifies container exists after creation
#[ignore]
#[tokio::test]
async fn test_create_container() -> Result<()> {
    let test_suite_name = "test-create-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure the container does not exist before we attempt to create it
    let container_exists = env
        .object_store_exists(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should check that '{test_container_name}' does not exist @ line {}",
                line!()
            )
        })?;
    assert!(!container_exists);

    // Invoke `wrpc:blobstore/blobstore.create-container`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::create_container(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await?
    .with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res {
        Ok(()) => (), // creation succeeded
        Err(e) => panic!("Failed to create container: {}", e),
    }

    // Ensure the container does exist after we attempted to create it
    let container_exists = env
        .object_store_exists(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should check that '{test_container_name}' does exist @ line {}",
                line!()
            )
        })?;
    assert!(container_exists);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests retrieving metadata about a container
///
/// Flow:
/// 1. Creates a container
/// 2. Gets container info, using the provider's `get-container-info` API
/// 3. Verifies returned metadata (creation time is Unix epoch since NATS doesn't store creation time)
#[ignore]
#[tokio::test]
async fn test_get_container_info() -> Result<()> {
    let test_suite_name = "test-get-container-info";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to get its info
    env.create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Invoke `wrpc:blobstore/blobstore.get-container-info`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::get_container_info(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await?
    .with_context(|| {
        format!(
            "should get container info for '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    assert!(res.is_ok());

    // NATS doesn't store creation time; so, the provider always returns Unix epoch, which can be verified
    let meta = res.unwrap();
    assert_eq!(meta.created_at, 0);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests checking for container existence
///
/// Flow:
/// 1. Verifies container doesn't exist initially
/// 2. Creates a container
/// 3. Verifies container exists after creation, using the provider's `container-exists` API
#[ignore]
#[tokio::test]
async fn test_container_exists() -> Result<()> {
    let test_suite_name = "test-container-exists";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Clean up any existing container first
    env.delete_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should cleanup by deleting container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // First verify container doesn't exist
    let container_exists = env
        .object_store_exists(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should check that '{test_container_name}' does not exist @ line {}",
                line!()
            )
        })?;
    assert!(!container_exists);

    // Then create the container
    env.create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Ensure the container exists
    let res_container_exists = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::container_exists(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await?
    .with_context(|| {
        format!(
            "should check that container '{test_container_name}' exists @ line {}",
            line!()
        )
    })?;
    assert!(res_container_exists.is_ok());
    assert!(res_container_exists.unwrap());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests retrieving blob data from a container
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Gets blob data using the provider's `get-container-data` API
/// 3. Verifies retrieved data matches original content
#[ignore]
#[tokio::test]
async fn test_get_container_data() -> Result<()> {
    let test_suite_name = "test-get-container-data";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure container and the blob inside the container exist
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| format!("should create blob '{test_blob_name}' @ line {}", line!()))?;

    let test_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.get-container-data`
    let (Ok((mut container_data_stream, _overall_result)), io) = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::get_container_data(&wrpc, env.wrpc_context(), &test_object, 0, 100),
    )
    .await?
    .with_context(|| {
        format!(
            "should get container data for '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?
    else {
        panic!("did not get results")
    };

    // Collect the data from the stream
    let mut stored_data = String::new();
    while let Some(data) = container_data_stream.next().await {
        stored_data.push_str(std::str::from_utf8(&data).unwrap_or_default());
    }
    if let Some(io) = io {
        io.await.context("failed to complete async I/O")?;
    }
    assert_eq!(stored_data, test_blob_body);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests writing blob data to a container
///
/// Flow:
/// 1. Creates a container
/// 2. Writes blob data using the provider's `write-container-data` API
/// 3. Verifies written data by reading it back directly from NATS
#[ignore]
#[tokio::test]
async fn test_write_container_data() -> Result<()> {
    let test_suite_name = "test-write-container-data";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to copy objects in it
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    let test_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    let input = Box::pin(stream::once(async {
        Bytes::from(test_blob_body.to_string())
    }));

    // Write the blob
    let (res, io) = blobstore::write_container_data(&wrpc, env.wrpc_context(), &test_object, input)
        .await
        .inspect_err(|e| println!("write operation failed: {}", e))?;
    assert!(res.is_ok());

    // Wait for IO completion with timeout
    if let Some(io) = io {
        tokio::time::timeout(Duration::from_secs(5), io)
            .await
            .context("IO completion timeout")?
            .context("IO operation failed")?;
    }

    // Ensure that the blob test_blob_name exist in test_container_name container, and has the content we wrote
    let mut nats_object = nats_container.get(test_blob_name).await.with_context(|| {
        format!(
            "should check whether '{test_blob_name}' exists in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    // Object implements `tokio::io::AsyncRead`.
    let mut blob_contents = vec![];
    nats_object.read_to_end(&mut blob_contents).await?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests listing blobs in a container
///
/// Flow:
/// 1. Creates a container and multiple test blobs
/// 2. Lists objects using the provider's `list-container-objects` API
/// 3. Verifies all created blobs are listed
#[ignore]
#[tokio::test]
async fn test_list_container_objects() -> Result<()> {
    let test_suite_name = "test-list-container-objects";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let mut test_blob_names = (1..=3)
        .map(|blob_id| format!("{test_blob_name}-{:0>3}", blob_id))
        .collect::<Vec<_>>();

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and blobs exists before listing them
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Create the blobs to be listed
    for blob_name in test_blob_names.clone() {
        nats_container
            .put(blob_name.as_str(), &mut test_blob_body.as_bytes())
            .await
            .with_context(|| {
                format!(
                    "should create blob '{blob_name}' in '{test_container_name}' @ line {}",
                    line!()
                )
            })?;
    }

    // Invoke `wrpc:blobstore/blobstore.list-container-objects`
    let (Ok((mut list_objects, _overall_result)), io) = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::list_container_objects(
            &wrpc,
            env.wrpc_context(),
            test_container_name,
            None,
            None,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should list objects in container '{test_container_name}' @ line {}",
            line!()
        )
    })?
    else {
        panic!("did not get results")
    };

    // Collect the objects from the stream
    let mut objects = Vec::new();
    while let Some(obj) = list_objects.next().await {
        objects.extend(obj);
    }
    if let Some(io) = io {
        io.await.context("failed to complete async I/O")?;
    }

    objects.sort();
    test_blob_names.sort();

    assert_eq!(objects, test_blob_names);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests clearing all blobs from a container
///
/// Flow:
/// 1. Creates a container and verifies it's empty
/// 2. Creates a test blob and verifies it exists
/// 3. Clears container using the provider's `clear-container` API
/// 4. Verifies container is empty again
#[ignore]
#[tokio::test]
async fn test_clear_container() -> Result<()> {
    let test_suite_name = "test-clear-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to clear it
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Ensure we have zero items to begin with
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        let obj = object?;
        objects.push(obj);
    }
    assert_eq!(objects.len(), 0);

    // Create a test blob named test_blob_name inside the test_container_name container
    nats_container
        .put(test_blob_name, &mut test_blob_body.as_bytes())
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Ensure we have a blob stored inside of the container
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        let obj = object?;
        objects.push(obj);
    }
    assert_eq!(objects.len(), 1);

    // Invoke `wrpc:blobstore/blobstore.clear-container`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::clear_container(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await?
    .with_context(|| {
        format!(
            "should clear container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res {
        Ok(()) => println!("Clear operation succeeded"),
        Err(e) => panic!("Failed to clear container: {}", e),
    }

    // Add a longer delay to allow clearing to complete
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Ensure the container is cleared
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        let obj = object?;
        objects.push(obj);
    }
    assert_eq!(objects.len(), 0);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests deleting a container
///
/// Flow:
/// 1. Creates a container and verifies it exists
/// 2. Deletes container using the provider's `delete-container` API
/// 3. Verifies container no longer exists
#[ignore]
#[tokio::test]
async fn test_delete_container() -> Result<()> {
    let test_suite_name = "test-delete-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to delete it
    let _nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let container_exists = env
        .object_store_exists(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should check that '{test_container_name}' does exist @ line {}",
                line!()
            )
        })?;
    assert!(container_exists);

    // Invoke `wrpc:blobstore/blobstore.delete-container`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_container(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await?
    .with_context(|| {
        format!(
            "should delete container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res {
        Ok(()) => (), // deletion succeeded
        Err(e) => panic!("Failed to delete container: {}", e),
    }

    // Ensure that the container does not exist after we attempted to delete it
    let container_exists = env
        .object_store_exists(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should check that '{test_container_name}' does exist @ line {}",
                line!()
            )
        })?;
    assert!(!container_exists);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests checking for blob existence in a container
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Checks blob existence using the provider's `has-object` API
/// 3. Verifies the API returns true for existing blob
#[ignore]
#[tokio::test]
async fn test_has_object() -> Result<()> {
    let test_suite_name = "test-has-object";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and the blob inside it exist before we check for its existence
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    let test_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.has-object`
    let res_has_object = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::has_object(&wrpc, env.wrpc_context(), &test_object),
    )
    .await?
    .with_context(|| {
        format!(
            "should check existence of blob '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res_has_object {
        Ok(exists) => assert!(exists),
        Err(e) => panic!("Failed to check object existence: {}", e),
    }

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests retrieving metadata about a blob
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Gets blob metadata using the provider's `get-object-info` API
/// 3. Verifies returned metadata matches NATS object info (size and creation time)
#[ignore]
#[tokio::test]
async fn test_get_object_info() -> Result<()> {
    let test_suite_name = "test-get-object-info";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and the blob inside it exist before we get its info
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Get NATS object metadata for comparison
    let nats_object_meta = nats_container
        .info(test_blob_name)
        .await
        .with_context(||format!("should get nats_object_meta for blob '{test_blob_name}' in '{test_container_name}' @ line {}", line!()))
        .map(
            |object_info| wrpc_interface_blobstore::bindings::wrpc::blobstore::types::ObjectMetadata {
                // NATS doesn't store the object creation time, so always return the Unix epoch
                created_at: 0,
                size: object_info.size as u64,
            },
        )
        .map_err(|e| anyhow::anyhow!(e))?;

    let test_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.get-object-info`
    let res_get_object_info = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::get_object_info(&wrpc, env.wrpc_context(), &test_object),
    )
    .await?
    .with_context(|| {
        format!(
            "should get object info for '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res_get_object_info {
        Ok(meta) => {
            assert_eq!(meta.size, nats_object_meta.size);
            assert_eq!(meta.created_at, nats_object_meta.created_at);
        }
        Err(e) => panic!("Failed to get object info: {}", e),
    }

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests copying a blob within the same container
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Copies blob to new name using the provider's `copy-object` API
/// 3. Verifies both source and destination blobs exist with correct content
#[ignore]
#[tokio::test]
async fn test_copy_object_within_container() -> Result<()> {
    let test_suite_name = "test-copy-object-within-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_copy_name = "test.blob.copy";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure container and the blob inside the container exist before we attempt to copy objects in it
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| format!("should create blob '{test_blob_name}' @ line {}", line!()))?;

    // Setup source and destination object references
    let source_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_copy_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.copy-object`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::copy_object(
            &wrpc,
            env.wrpc_context(),
            &source_object,
            &destination_object,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should copy blob '{test_blob_name}' to '{test_blob_copy_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res {
        Ok(()) => (), // copy succeeded
        Err(e) => panic!("Failed to copy object: {}", e),
    }

    // Ensure the destination blob exists and has the content of the source blob
    let mut nats_object = nats_container.get(test_blob_name).await.with_context(|| {
        format!(
            "should check whether '{test_blob_name}' exists in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    // Object implements `tokio::io::AsyncRead`.
    let mut blob_copy_contents = vec![];
    nats_object.read_to_end(&mut blob_copy_contents).await?;
    assert_eq!(blob_copy_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests copying a blob between two different containers
///
/// Flow:
/// 1. Creates source and destination containers
/// 2. Creates test blob in source container
/// 3. Copies blob using the provider's `copy-object` API
/// 4. Verifies blob exists in both source and destination with correct content
#[ignore]
#[tokio::test]
async fn test_copy_object_across_containers() -> Result<()> {
    let test_suite_name = "test-copy-object-across-containers";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let source_name = &format!("{test_container_name}-source");
    let destination_name = &format!("{test_container_name}-destination");

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the source container exists before we attempt to copy its objects to the destination container
    let nats_source_container = env
        .create_object_store(source_name)
        .await
        .with_context(|| format!("should create container '{source_name}' @ line {}", line!()))?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_source_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| format!("should create blob '{test_blob_name}' @ line {}", line!()))?;

    // Ensure that the destination container exists before we attempt to copy objects to it
    let nats_destination_container = env
        .create_object_store(destination_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{destination_name}' @ line {}",
                line!()
            )
        })?;

    // Setup source and destination object references
    let source_object = ObjectId {
        container: source_name.to_string(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: destination_name.to_string(),
        object: test_blob_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.copy-object`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::copy_object(
            &wrpc,
            env.wrpc_context(),
            &source_object,
            &destination_object,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should copy blob '{test_blob_name}' from '{source_name}' to '{destination_name}' @ line {}",
            line!()
        )
    })?;
    assert!(res.is_ok());

    // Ensure that the blob does not exist in source container after move
    let source_blob_exist = match nats_source_container.get(test_blob_name).await {
        Ok(_) => true,
        Err(e) => match e.kind() {
            async_nats::jetstream::object_store::GetErrorKind::NotFound => false,
            _ => {
                return Err(e).context(format!(
                    "should check whether '{test_blob_name}' exists in '{source_name}' @ line {}",
                    line!()
                ))
            }
        },
    };
    assert!(!source_blob_exist);

    // Ensure that the destination blob exists and has the expected contents
    let mut destination_blob = nats_destination_container
        .get(test_blob_name)
        .await
        .with_context(|| {
            format!(
                "should get contents of '{test_blob_name}' in '{destination_name}' @ line {}",
                line!()
            )
        })?;
    let mut blob_contents = vec![];
    destination_blob.read_to_end(&mut blob_contents).await?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Cleanup
    env.delete_object_store(source_name)
        .await
        .with_context(|| {
            format!(
                "should cleanup by deleting container '{source_name}' @ line {}",
                line!()
            )
        })?;
    env.delete_object_store(destination_name)
        .await
        .with_context(|| {
            format!(
                "should cleanup by deleting container '{destination_name}' @ line {}",
                line!()
            )
        })?;

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests moving a blob within the same container
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Moves blob to new name using the provider's `move-object` API
/// 3. Verifies source blob no longer exists
/// 4. Verifies destination blob exists with correct content
#[ignore]
#[tokio::test]
async fn test_move_object_within_container() -> Result<()> {
    let test_suite_name = "test-move-object-within-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_name_move = "test.blob.move";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and the blob inside it exist before we attempt to move the blob
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    let source_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name_move.to_string(),
    };

    // Invoke `wrpc:blobstore/blobstore.move-object`
    let res_move_object = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::move_object(
            &wrpc,
            env.wrpc_context(),
            &source_object,
            &destination_object,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should move blob '{test_blob_name}' to '{test_blob_name_move}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    assert!(res_move_object.is_ok());

    // If the source and destination are not the same, ensure that the source blob does not exist after it is moved to destination
    let source_blob_exist = match nats_container.get(test_blob_name).await {
        Ok(_) => true,
        Err(e) => match e.kind() {
            async_nats::jetstream::object_store::GetErrorKind::NotFound => false,
            _ => return Err(e).context(format!(
                "should check whether '{test_blob_name}' exists in '{test_container_name}' @ line {}",
                line!()
            ))
        }
    };
    assert!(!source_blob_exist);

    // Ensure that the destination blob exists and has the content of the source blob
    let destination_blob_exist =
        nats_container
            .get(test_blob_name_move)
            .await
            .with_context(|| {
                format!(
                "should get contents of '{test_blob_name_move}' in '{test_container_name}' @ line {}",
                line!()
            )
            })?;
    let mut destination_blob = destination_blob_exist;
    let mut blob_contents = vec![];
    destination_blob.read_to_end(&mut blob_contents).await?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests moving a blob to itself (should be a no-op)
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Attempts to move blob to same name using the provider's `move-object` API
/// 3. Verifies blob still exists with original content (move to self is no-op)
#[ignore]
#[tokio::test]
async fn test_move_object_to_itself() -> Result<()> {
    let test_suite_name = "test-move-object-to-itself";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_name_move = test_blob_name;
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and the blob inside it exist before we attempt to move the blob
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    let source_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name_move.to_string(),
    };

    // Invoke `wrpc:blobstore/blobstore.move-object`
    let res_move_object = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::move_object(
            &wrpc,
            env.wrpc_context(),
            &source_object,
            &destination_object,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should move blob '{test_blob_name}' to '{test_blob_name_move}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    assert!(res_move_object.is_ok());

    // A move between different object_ids will result in the source blob being deleted; so, ensure the source blob is intact
    let source_blob_exist = nats_container.get(test_blob_name).await.with_context(|| {
        format!(
            "should get contents of '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    let mut source_blob = source_blob_exist;
    let mut blob_contents = vec![];
    source_blob.read_to_end(&mut blob_contents).await?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests moving a blob across different containers
///
/// Flow:
/// 1. Creates source and destination containers
/// 2. Creates test blob in source container
/// 3. Moves blob using the provider's `move-object` API
/// 4. Verifies source blob no longer exists
/// 5. Verifies destination blob exists with correct content
#[ignore]
#[tokio::test]
async fn test_move_object_across_containers() -> Result<()> {
    let test_suite_name = "test-move-object-across-containers";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let source_name = &format!("{test_container_name}-source");
    let destination_name = &format!("{test_container_name}-destination");

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Create source and destination containers
    let nats_source_container = env
        .create_object_store(source_name)
        .await
        .with_context(|| format!("should create container '{source_name}' @ line {}", line!()))?;
    let nats_destination_container = env
        .create_object_store(destination_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{destination_name}' @ line {}",
                line!()
            )
        })?;

    // Create test blob in source container
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_source_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| format!("should create blob '{test_blob_name}' @ line {}", line!()))?;

    let source_object = ObjectId {
        container: source_name.to_owned(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: destination_name.to_owned(),
        object: test_blob_name.to_string(),
    };

    // Invoke `wrpc:blobstore/blobstore.move-object`
    let res_move_object = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::move_object(
            &wrpc,
            env.wrpc_context(),
            &source_object,
            &destination_object,
        ),
    )
    .await?
    .with_context(|| {
        format!(
            "should move blob '{test_blob_name}' from '{source_name}' to '{destination_name}' @ line {}",
            line!()
        )
    })?;
    assert!(res_move_object.is_ok());

    // Ensure that the blob does not exist in source container after move
    let source_blob_exist = match nats_source_container.get(test_blob_name).await {
        Ok(_) => true,
        Err(e) => match e.kind() {
            async_nats::jetstream::object_store::GetErrorKind::NotFound => false,
            _ => {
                return Err(e).context(format!(
                    "should check whether '{test_blob_name}' exists in '{source_name}' @ line {}",
                    line!()
                ))
            }
        },
    };
    assert!(!source_blob_exist);

    // Ensure that the destination blob exists and has the expected contents
    let mut destination_blob = nats_destination_container
        .get(test_blob_name)
        .await
        .with_context(|| {
            format!(
                "should get contents of '{test_blob_name}' in '{destination_name}' @ line {}",
                line!()
            )
        })?;
    let mut blob_contents = vec![];
    destination_blob.read_to_end(&mut blob_contents).await?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Cleanup
    env.delete_object_store(source_name)
        .await
        .with_context(|| {
            format!(
                "should cleanup by deleting container '{source_name}' @ line {}",
                line!()
            )
        })?;
    env.delete_object_store(destination_name)
        .await
        .with_context(|| {
            format!(
                "should cleanup by deleting container '{destination_name}' @ line {}",
                line!()
            )
        })?;

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests deleting a blob from a container
///
/// Flow:
/// 1. Creates a container and test blob
/// 2. Deletes blob using the provider's `delete-object` API
/// 3. Verifies blob no longer exists
#[ignore]
#[tokio::test]
async fn test_delete_object() -> Result<()> {
    let test_suite_name = "test-delete-object";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to create blobs in it
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Ensure that the blob exists before we attempt to delete it
    let mut reader = tokio::io::BufReader::new(test_blob_body.as_bytes());
    nats_container
        .put(test_blob_name, &mut reader)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Verify blob exists before deletion
    match nats_container.get(test_blob_name).await {
        Ok(_) => println!("Blob exists before deletion"),
        Err(e) => println!("Error checking blob before deletion: {}", e),
    }

    let test_object = ObjectId {
        container: test_container_name.to_string(),
        object: test_blob_name.to_string(),
    };
    // Invoke `wrpc:blobstore/blobstore.delete-object`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_object(&wrpc, env.wrpc_context(), &test_object),
    )
    .await?
    .with_context(|| {
        format!(
            "should delete blob '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    match res {
        Ok(()) => println!("Delete operation succeeded"),
        Err(e) => panic!("Failed to delete object: {}", e),
    }

    // Add a longer delay to allow deletion to complete
    tokio::time::sleep(Duration::from_millis(500)).await; // Increased delay

    // Ensure that the blob does not exist after it is deleted
    let test_blob_exists = match nats_container.get(test_blob_name).await {
        Ok(_) => {
            println!("Error: Blob still exists after deletion");
            true
        },
        Err(e) => match e.kind() {
            async_nats::jetstream::object_store::GetErrorKind::NotFound => {
                println!("Success: Blob was deleted");
                false
            },
            _ => return Err(e).with_context(|| format!(
                "should check existence of blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            ))
        }
    };
    assert!(!test_blob_exists);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

/// Tests deleting multiple blobs from a container
///
/// Flow:
/// 1. Creates a container and multiple test blobs
/// 2. Deletes all blobs using the provider's `delete-objects` API
/// 3. Verifies all blobs are removed from container
#[ignore]
#[tokio::test]
async fn test_delete_objects() -> Result<()> {
    let test_suite_name = "test-delete-objects";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_body = test_suite_name;
    let test_blob_names = (1..=3)
        .map(|blob_id| format!("test.blob-{:0>3}", blob_id))
        .collect::<Vec<_>>();

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and wait longer to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let wrpc = env.wrpc_client().await?;

    // Create the container and test blobs
    let nats_container = env
        .create_object_store(test_container_name)
        .await
        .with_context(|| {
            format!(
                "should create container '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    // Verify container is empty before we start
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        let obj = object?;
        objects.push(obj);
    }
    assert_eq!(objects.len(), 0, "Container should be empty before test");

    // Create the blobs to be deleted
    for blob_name in test_blob_names.clone() {
        nats_container
            .put(blob_name.as_str(), &mut test_blob_body.as_bytes())
            .await
            .with_context(|| {
                format!(
                    "should create blob '{blob_name}' in '{test_container_name}' @ line {}",
                    line!()
                )
            })?;
    }

    // Verify we have exactly 3 blobs
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        objects.push(object?);
    }
    assert_eq!(objects.len(), test_blob_names.len()); // Should be 3

    // Invoke `wrpc:blobstore/blobstore.delete-objects`
    let res_delete_objects = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_objects(
            &wrpc,
            env.wrpc_context(),
            test_container_name,
            &test_blob_names
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
        ),
    )
    .await?
    .with_context(|| format!("should delete objects @ line {}", line!()))?;
    assert!(res_delete_objects.is_ok());

    // Ensure all blobs were deleted
    let mut list_stream = nats_container.list().await?;
    let mut objects = Vec::new();
    while let Some(object) = list_stream.next().await {
        objects.push(object?);
    }
    assert_eq!(objects.len(), 0);

    // Shutdown
    provider_handle.abort();

    Ok(())
}
