use anyhow::{Context as _, Result};
use bytes::Bytes;
use futures::{stream, StreamExt as _};
use std::{collections::HashMap, time::Duration};
use tokio::try_join;
use wasmcloud_provider_blobstore_azure::BlobstoreAzblobProvider;
use wasmcloud_provider_sdk::{
    get_connection, provider::initialize_host_data, run_provider, serve_provider_exports, HostData,
    InterfaceLinkDefinition,
};
use wasmcloud_test_util::testcontainers::{
    AsyncRunner as _, Azurite, ContainerAsync, ImageExt, NatsServer,
};
use wrpc_interface_blobstore::bindings::{
    serve,
    wrpc::blobstore::{blobstore, types::ObjectId},
};

struct TestEnv {
    _azurite: ContainerAsync<Azurite>,
    _nats: ContainerAsync<NatsServer>,
    azurite_address: String,
    nats_address: String,
    lattice: String,
    test_suite: String,
}

impl TestEnv {
    pub async fn new(lattice: &str, test_suite: &str) -> Result<Self> {
        let azurite = Azurite::default()
            .start()
            .await
            .context("should start azurite")?;
        let azurite_ip = azurite
            .get_host()
            .await
            .context("should get azurite host ip")?;
        let azurite_port = azurite
            .get_host_port_ipv4(10000)
            .await
            .context("should get azurite host port")?;
        let azurite_address = format!("{azurite_ip}:{azurite_port}");

        let nats = NatsServer::default()
            .with_startup_timeout(Duration::from_secs(15))
            .start()
            .await
            .context("should start nats-server")?;
        let nats_ip = nats
            .get_host()
            .await
            .context("should get nats-server host ip")?;
        let nats_port = nats
            .get_host_port_ipv4(4222)
            .await
            .context("should get nats-server host port")?;
        let nats_address = format!("{nats_ip}:{nats_port}");

        let host_data = HostData {
            lattice_rpc_url: nats_address.clone(),
            lattice_rpc_prefix: lattice.to_string(),
            provider_key: test_suite.to_string(),
            config: HashMap::new(),
            link_definitions: vec![InterfaceLinkDefinition {
                source_id: "test-component".to_string(),
                target: test_suite.to_string(),
                name: test_suite.to_string(),
                wit_namespace: "wrpc".to_string(),
                wit_package: "blobstore".to_string(),
                interfaces: vec!["blobstore".to_string()],
                source_config: HashMap::new(),
                target_config: HashMap::from([
                    ("CLOUD_LOCATION".to_string(), Self::azurite_endpoint(&azurite_address)),
                    // https://learn.microsoft.com/en-us/azure/storage/common/storage-use-azurite?tabs=docker-hub%2Cblob-storage#well-known-storage-account-and-key
                    ("STORAGE_ACCOUNT".to_string(), "devstoreaccount1".to_string()),
                    ("STORAGE_ACCESS_KEY".to_string(), "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==".to_string()),
                ]),
                source_secrets: None,
                target_secrets: None,
            }],
            ..Default::default()
        };
        initialize_host_data(host_data.clone()).expect("should be able to initialize host data");

        Ok(Self {
            lattice: lattice.to_string(),
            test_suite: test_suite.to_string(),
            azurite_address,
            nats_address,
            _azurite: azurite,
            _nats: nats,
        })
    }

    // Uses the emulator path with custom port: https://github.com/Azure/azure-sdk-for-rust/blob/v2024-04-24/sdk/storage/src/cloud_location.rs#L46-L48
    pub fn azurite_endpoint(address: &str) -> String {
        format!("http://{}/devstoreaccount1", address)
    }

    pub fn azurite_blob_client(&self) -> azure_storage_blobs::prelude::BlobServiceClient {
        let (address, port) = self.azurite_address.split_once(":").unwrap_or(("", ""));
        let location = azure_storage::CloudLocation::Emulator {
            address: address.to_string(),
            port: port.parse::<u16>().unwrap_or(10000),
        };
        let builder = azure_storage_blobs::prelude::ClientBuilder::with_location(
            location,
            azure_storage::StorageCredentials::emulator(),
        );
        builder.blob_service_client()
    }

    pub fn nats_endpoint(address: &str) -> String {
        format!("nats://{}", address)
    }

    async fn nats_client(&self) -> Result<async_nats::Client> {
        async_nats::ConnectOptions::new()
            .name("test-nats-client")
            .connect(Self::nats_endpoint(&self.nats_address))
            .await
            .map_err(anyhow::Error::msg)
    }

    pub async fn start_provider(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        // TODO: we should not need the `cfg!(debug_assertions)` check here, but until
        // we resolve the `stack overflow` resulting from `tracing`` instrument calls
        // inside wrpc, `BlobstoreAzblobProvider::run` can only be used to run the provider
        // when the `--release` is passed (to `cargo test`).
        let handle = if cfg!(debug_assertions) {
            let provider = BlobstoreAzblobProvider::default();
            let shutdown = run_provider(provider.clone(), "blobstore-azure-provider")
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
                BlobstoreAzblobProvider::run()
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
        headers.insert("link-name", "blobstore-provider-azure");
        Some(headers)
    }
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container_client = env
        .azurite_blob_client()
        .container_client(test_container_name);

    container_client.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    let blob_count = container_client
        .list_blobs()
        .into_stream()
        .map(|r| r.unwrap().blobs.items.len())
        .collect::<Vec<_>>()
        .await;

    // Ensure we have zero items to begin with
    assert_eq!(*blob_count.first().unwrap(), 0);

    // Create a test blob named `test.blob` inside the `{test_container_name}` container and put the word `test` inside of it.
    let blob_client = container_client.blob_client(test_blob_name);
    blob_client
        .put_block_blob(test_blob_body)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;

    let blob_count = container_client
        .list_blobs()
        .into_stream()
        .map(|r| r.unwrap().blobs.items.len())
        .collect::<Vec<_>>()
        .await;

    // Ensure we have a blob stored inside of the container
    assert_eq!(*blob_count.first().unwrap(), 1);

    // Invoke `wrpc:blobstore/blobstore.clear-container`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::clear_container(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await??;
    assert!(res.is_ok());

    let blob_count = container_client
        .list_blobs()
        .into_stream()
        .map(|r| r.unwrap().blobs.items.len())
        .collect::<Vec<_>>()
        .await;

    // Ensure we have zero items in the container
    assert_eq!(*blob_count.first().unwrap(), 0);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_container_exists() -> Result<()> {
    let test_suite_name = "test-container-exists";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    // Invoke `wrpc:blobstore/blobstore.container-exists`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::container_exists(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await??;

    assert!(res.is_ok());
    assert!(!res.unwrap());

    let container_client = env
        .azurite_blob_client()
        .container_client(test_container_name);
    container_client.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    // blobstore.container_exists returns true when queried against existing container
    let res_container_exists = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::container_exists(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await??;

    assert!(res_container_exists.is_ok());
    assert!(res_container_exists.unwrap());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_create_container() -> Result<()> {
    let test_suite_name = "test-create-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure the container does not exist before we attempt to create it
    let container_exists = container.exists().await.with_context(|| {
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
    .await??;
    assert!(res.is_ok());

    // Ensure the container does exist after we attempted to create it
    let container_exists = container.exists().await.with_context(|| {
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

#[ignore]
#[tokio::test]
async fn test_delete_container() -> Result<()> {
    let test_suite_name = "test-delete-container";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to delete it
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    let container_exists = container.exists().await.with_context(|| {
        format!(
            "should check that container '{test_container_name}' exists @ line {}",
            line!()
        )
    })?;
    assert!(container_exists);

    // Invoke `wrpc:blobstore/blobstore.delete-container`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_container(&wrpc, env.wrpc_context(), test_container_name),
    )
    .await??;
    assert!(res.is_ok());

    // Ensure that the container does not exist after we attempted to delete it
    let container_exists = container.exists().await.with_context(|| {
        format!(
            "should check that container '{test_container_name}' does not exist @ line {}",
            line!()
        )
    })?;
    assert!(!container_exists);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_get_container_info() -> Result<()> {
    let test_suite_name = "test-get-container-info";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to delete it
    container.create().await.with_context(|| {
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
    .await??;

    assert!(res.is_ok());
    // TODO: Add test to verify that the returned timestamp approximates creation time, or some known default.

    // Shutdown
    provider_handle.abort();

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_list_container_objects() -> Result<()> {
    let test_suite_name = "test-list-container-objects";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let mut test_blob_names = (1..=3)
        .map(|blob_id| format!("{test_blob_name}.{:0>3}", blob_id))
        .collect::<Vec<_>>();

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container and blobs exists before listing them
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    // Create the blobs to be listed
    for blob_name in test_blob_names.clone() {
        let blob_client = container.blob_client(&blob_name);
        let _ = blob_client
            .put_block_blob(test_blob_body)
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
    .await??
    else {
        panic!("did not get results")
    };

    // TODO: Simplify this
    let (_, mut objects) = try_join!(
        async {
            if let Some(io) = io {
                io.await.context("failed to complete async I/O")
            } else {
                Err(anyhow::anyhow!("failed to drive async i/o"))
            }
        },
        async {
            let mut objects = Vec::new();
            while let Some(obj) = list_objects.next().await {
                objects.extend(obj);
            }
            Ok(objects)
        }
    )?;

    objects.sort();
    test_blob_names.sort();

    assert_eq!(objects, test_blob_names);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let client = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to copy objects in it
    client.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    let blob_client = client.blob_client(test_blob_name);
    blob_client
        .put_block_blob(test_blob_body)
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
    .await??;
    assert!(res.is_ok());

    // Ensure that the container does not exist after we attempted to delete it
    let blob_copy_contents = client
        .blob_client(test_blob_copy_name)
        .get_content()
        .await
        .with_context(|| format!("should get contents of '{test_blob_copy_name}' in '{test_container_name}' @ line {}", line!()))?;
    assert_eq!(blob_copy_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let source = env.azurite_blob_client().container_client(source_name);
    let destination = env.azurite_blob_client().container_client(destination_name);

    // Ensure that the container exists before we attempt to copy objects in it
    source
        .create()
        .await
        .with_context(|| format!("should create container '{source_name}' @ line {}", line!()))?;
    destination.create().await.with_context(|| {
        format!(
            "should create container '{destination_name}' @ line {}",
            line!()
        )
    })?;
    source
        .blob_client(test_blob_name)
        .put_block_blob(test_blob_body)
        .await
        .with_context(|| {
            format!(
                "should create blob '{test_blob_name}' in '{source_name}' @ line {}",
                line!()
            )
        })?;

    // Setup source and destination object references
    let source_object = ObjectId {
        container: source.container_name().to_string(),
        object: test_blob_name.to_string(),
    };
    let destination_object = ObjectId {
        container: destination.container_name().to_string(),
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
    .await??;
    assert!(res.is_ok());

    // Ensure that the blob exists and has the content we wrote initially
    let destination_blob_contents = destination
        .blob_client(test_blob_name)
        .get_content()
        .await
        .with_context(|| {
            format!(
                "should query blob '{test_blob_name}' in '{destination_name}' @ line {}",
                line!()
            )
        })?;
    assert_eq!(destination_blob_contents, test_suite_name.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to create blobs in it
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    // Ensure that the blob exists before we attempt to delete it
    let blob_client = container.blob_client(test_blob_name);
    blob_client
        .put_block_blob(test_blob_body)
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
    // Invoke `wrpc:blobstore/blobstore.delete-object`
    let res = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_object(&wrpc, env.wrpc_context(), &test_object),
    )
    .await??;
    assert!(res.is_ok());

    // Ensure that the blob does not exist after it is deleted
    let test_blob_exists = blob_client.exists().await.with_context(|| {
        format!(
            "should check whether '{test_blob_name}' exists @ line {}",
            line!()
        )
    })?;
    assert!(!test_blob_exists);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

#[ignore]
#[tokio::test]
async fn test_delete_objects() -> Result<()> {
    let test_suite_name = "test-delete-objects";
    let test_container_name = test_suite_name;
    let lattice_name = "default";
    let test_blob_name = "test.blob";
    let test_blob_body = test_suite_name;
    let test_blob_names = (1..=3)
        .map(|blob_id| format!("{test_blob_name}-{:0>3}", blob_id))
        .collect::<Vec<_>>();

    let env = TestEnv::new(lattice_name, test_suite_name)
        .await
        .with_context(|| format!("should setup the test environment @ line {}", line!()))?;

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to create blobs in it
    container
        .create()
        .await
        .with_context(|| format!("should create container /'{test_container_name}'"))?;

    // Create the blobs to be deleted
    for blob_name in test_blob_names.clone() {
        let blob_client = container.blob_client(blob_name);
        let _ = blob_client
            .put_block_blob(test_blob_body)
            .await
            .with_context(|| {
                format!("should create blob '{test_blob_name}' in '{test_container_name}'")
            })?;
    }

    // Ensure we have expected number of blobs to begin with
    let blob_count = container
        .list_blobs()
        .into_stream()
        .map(|r| r.unwrap().blobs.items.len())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(*blob_count.first().unwrap(), test_blob_names.len());

    // Invoke `wrpc:blobstore/blobstore.delete-objects`
    let res_delete_objects = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::delete_objects(
            &wrpc,
            env.wrpc_context(),
            test_container_name,
            &test_blob_names
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>(),
        ),
    )
    .await??;
    assert!(res_delete_objects.is_ok());

    // Ensure all blobs were deleted
    let blob_count = container
        .list_blobs()
        .into_stream()
        .map(|r| r.unwrap().blobs.items.len())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(*blob_count.first().unwrap(), 0);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure container and the blob inside the container exist
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    container
        .blob_client(test_blob_name)
        .put_block_blob(test_blob_body)
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
    // Invoke `wrpc:blobstore/blobstore.get-container-data`
    let (Ok((mut container_data_stream, _overall_result)), io) = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::get_container_data(&wrpc, env.wrpc_context(), &test_object, 0, 100),
    )
    .await??
    else {
        panic!("did not get results")
    };

    // TODO: Simplify this
    let (_, stored_data) = try_join! {
        async {
            if let Some(io) = io {
                io.await.context("failed to complete async I/O")
            } else {
                Err(anyhow::anyhow!("failed to drive async i/o"))
            }
        },
        async {
            let mut res = String::new();
            while let Some(data) = container_data_stream.next().await {
                res.push_str(std::str::from_utf8(&data).unwrap_or_default());
            }
            Ok(res)
        },
    }?;

    assert_eq!(stored_data, test_blob_body);

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let client = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to copy objects in it
    client.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    let blob_client = client.blob_client(test_blob_name);
    blob_client
        .put_block_blob(test_blob_body)
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
    // Invoke `wrpc:blobstore/blobstore.get-object-info`
    let res_get_object_info = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::get_object_info(&wrpc, env.wrpc_context(), &test_object),
    )
    .await??;
    assert!(res_get_object_info.is_ok());

    // Ensure returned get-object-info matches up with values from Azure API
    let properties = blob_client
        .get_properties()
        .await
        .with_context(||format!("should get properties for blob '{test_blob_name}' in '{test_container_name}' @ line {}", line!()))?;

    let res_get_object_info = res_get_object_info.unwrap();
    assert_eq!(
        res_get_object_info.size,
        properties.blob.properties.content_length
    );
    assert_eq!(
        res_get_object_info.created_at,
        properties.blob.properties.creation_time.unix_timestamp() as u64
    );

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;
    let client = env
        .azurite_blob_client()
        .container_client(test_container_name);

    // Ensure that the container exists before we attempt to copy objects in it
    client.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;
    let blob_client = client.blob_client(test_blob_name);
    blob_client
        .put_block_blob(test_blob_body)
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
    .await??;
    assert!(res_has_object.is_ok());
    assert!(res_has_object.unwrap());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to copy objects in it
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    let blob = container.blob_client(test_blob_name);
    blob.put_block_blob(test_blob_body).await.with_context(|| {
        format!(
            "should create blob '{test_blob_name}' in '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    // Invoke the wrpc endpoint for deleting a container
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
    .await??;
    assert!(res_move_object.is_ok());

    // Ensure that the blob does not exist in the source location after it is moved to destination
    let source_blob_exist = container
        .blob_client(test_blob_name)
        .exists()
        .await
        .with_context(|| {
            format!("should check whether '{test_blob_name}' exists in '{test_container_name}' @ line {}", line!())
        })?;
    let destination_blob_exist = container
        .blob_client(test_blob_name_move)
        .get_content()
        .await
        .with_context(|| {
            format!(
                "should get contents of '{test_blob_name}' in '{test_container_name}' @ line {}",
                line!()
            )
        })?;
    assert!(!source_blob_exist);
    assert_eq!(destination_blob_exist, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    let source = env.azurite_blob_client().container_client(source_name);
    let destination = env.azurite_blob_client().container_client(destination_name);

    // Ensure that the container exists before we attempt to copy objects in it
    source
        .create()
        .await
        .with_context(|| format!("should create container '{source_name}' @ line {}", line!()))?;
    destination.create().await.with_context(|| {
        format!(
            "should create container '{destination_name}' @ line {}",
            line!()
        )
    })?;

    let blob = source.blob_client(test_blob_name);
    blob.put_block_blob(test_blob_body).await.with_context(|| {
        format!(
            "should create blob '{test_blob_name}' in '{source_name}' @ line {}",
            line!()
        )
    })?;

    // Invoke the wrpc endpoint for deleting a container
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
    .await??;
    assert!(res_move_object.is_ok());

    // Ensure that the blob does not exist in the source location after it is moved to destination
    let source_blob_exist = source
        .blob_client(test_blob_name)
        .exists()
        .await
        .with_context(|| {
            format!(
                "should check whether '{test_blob_name}' exists in '{source_name}' @ line {}",
                line!()
            )
        })?;
    assert!(!source_blob_exist);

    // Ensure that the destination blob has the expected contents
    let destination_blob_exist = destination
        .blob_client(test_blob_name)
        .get_content()
        .await
        .with_context(|| {
            format!(
                "should get contents of '{test_blob_name}' in '{destination_name}' @ line {}",
                line!()
            )
        })?;
    assert_eq!(destination_blob_exist, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}

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

    // Start the provider and things a second to settle
    let provider_handle = env.start_provider().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let wrpc = env.wrpc_client().await?;

    // Ensure that the container exists before we attempt to copy objects in it
    let container = env
        .azurite_blob_client()
        .container_client(test_container_name);
    container.create().await.with_context(|| {
        format!(
            "should create container '{test_container_name}' @ line {}",
            line!()
        )
    })?;

    // Create an existing blob at test_blob_name location
    container
        .blob_client(test_blob_name)
        .put_block_blob("")
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
    let input = Box::pin(stream::once(async {
        Bytes::from(test_blob_body.to_string())
    }));

    // Invoke `wrpc:blobstore/blobstore.write-container-data`
    let (res, io) = tokio::time::timeout(
        Duration::from_secs(1),
        blobstore::write_container_data(&wrpc, env.wrpc_context(), &test_object, input),
    )
    .await??;
    assert!(res.is_ok());
    if let Some(io) = io {
        io.await.with_context(|| {
            format!(
                "should complete i/o for 'blobstore.writing-container-data' @ line {}",
                line!()
            )
        })?;
    }

    // Ensure that the blob does not exist in the source location after it is moved to destination
    let blob_contents = container
       .blob_client(test_blob_name)
       .get_content()
       .await
       .with_context(|| {
           format!(
               "should check whether '{test_blob_name}' exists in '{test_container_name}' @ line {}",
               line!()
           )
       })?;
    assert_eq!(blob_contents, test_blob_body.as_bytes());

    // Shutdown
    provider_handle.abort();

    Ok(())
}
