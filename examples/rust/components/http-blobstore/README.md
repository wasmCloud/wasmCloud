# HTTP Blobstore Example Component

This example demonstrates the WASI Blobstore interface by performing various blob storage operations and exposing the results via HTTP. The component can work with any available blobstore providers, including S3, Azure blob, local filesystem, and NATS.

## What It Does

The component performs a series of blobstore operations:

1. Container Operations:
   - Creates two containers (default: "ying" and "yang")
   - Verifies container existence
   - Retrieves container metadata

2. Basic Blob Operations:
   - Writes four blobs ("earth", "air", "fire", "water") to "ying" container
   - Reads back and verifies content
   - Demonstrates partial content reading (first 4 bytes)

3. Advanced Operations:
   - Moves "fire" from "ying" to "yang" container
   - Copies "water" from "ying" to "yang" container
   - Lists objects in both containers
   - Cleans up by clearing "ying" container

## Building the Component

### Prerequisites

- [`wash`](https://wasmcloud.com/docs/installation) 0.26.0+
- `cargo` 1.75+

```bash
wash build
```

## Deploying the Component

You can use the `local-wadm.yaml` application manifest if you're planning to build and run the component, and the desired provider locally; or you can use `wadm.yaml` to deploy the pre-built components, from the wasmCloud package registry.

### With NATS Blobstore Provider

The NATS provider stores blobs in NATS JetStream. It's the provider that has been used in the aformentioned application manifest:

```bash
# Deploy the application with wash CLI
wash app deploy ./wadm.yaml

# Check the deployment status (`Deployed` status means the application is ready)
wash app get
#alternatively, run:
wash get inventory
```

For provider configuration options, see the [blobstore-nats provider documentation](../../provider-blobstore-nats/README.md).

### With Filesystem Blobstore Provider

The filesystem provider stores blobs in your local filesystem. To use it make sure it's added to the desired application manifest file. other than that, the steps are the same as the NATS provider.

For provider configuration options, see the [blobstore-fs provider documentation](../../provider-blobstore-fs/README.md).

#### Blobstore and Filesystem Storage

The blobstore container and blobs are stored in the `/tmp/<component_id>` folder. The component ID used is based on the public key of the component that we automatically generate for you, which you can find by inspecting the component:

```bash
wash inspect ./build/http_blobstore_s.wasm
```

If you're looking to extend this example to read, write, or list files, you can simply place them in the temporary directory above so that your component can access them.

## Testing the Component

Once deployed, run the component by issuing the following command:

```bash
curl http://localhost:8080
```

The component will return a JSON response showing the results of all operations:

```json
{
  "container_ops": {
    "create_container": {
      "success": true,
      "message": "Created ying",
      "timestamp": "2024-12-05 16:30:00 UTC"
    },
    "container_info": {
      "success": true,
      "message": "Container ying created at 2024-12-05 16:30:00 UTC",
      "timestamp": "2024-12-05 16:30:00 UTC"
    }
    // ... more container operations
  },
  "blob_ops": {
    "write_blob": {
      "success": true,
      "message": "Wrote earth to ying",
      "timestamp": "2024-12-05 16:30:01 UTC"
    }
    // ... more blob operations
  },
  "container_names": ["ying", "yang"]
}
```
