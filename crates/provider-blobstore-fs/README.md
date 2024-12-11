# `blobstore-fs` capability provider

This capability provider implements the `wasmcloud:blobstore` capability for Unix and Windows file
systems. 

The provider will store files and folders on the host where the provider executes, and expose those
folders and files within as a blobstore (AKA object storage).

## Configuration

Similar to other wasmcloud providers, this provider is configured with link configuration values:

| Link value | Default               | Example            | Description                               |
| ---------- | --------------------- | ------------------ | ----------------------------------------- |
| `ROOT`     | `/tmp/<component-id>` | `/tmp/your-folder` | The root folder where data will be stored |

The default value will create a folder in the `/tmp` directory with the name of the component ID so
as to avoid collision when linking multiple components

> [!NOTE]
> The provider must have read and write access to the disk location specified by `ROOT`

