# `blobstore-fs` capability provider

This capability provider implements the `wasmcloud:blobstore` capability for Unix and Windows file systems. 

The provider will store files and folders on the host where the provider executes, and expose those folders and files within as a blobstore (AKA object storage).

## Configuration

Similar to other wasmcloud providers, this provider is configured wiht link configuration values:

| Link value | Default | Example            | Description                               |
|------------|---------|--------------------|-------------------------------------------|
| `ROOT`     | `/tmp`  | `/tmp/your-folder` | The root folder where data will be stored |

> [!NOTE]
> The provider must have read and write access to the disk location specified by `ROOT`
>
> Each component's files will be stored under the path `$ROOT/<component id>`

