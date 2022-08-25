# Blobstore-Fs capability provider

This capability provider implements the `wasmcloud:blobstore` capability for
Unix and Windows file system. The provider will store files in the local host where the
provider executes.

## Building

Build with 'make'. Test with 'make test'.
Testing requires docker.

## Configuration

The provider is configured with the link configuration value `ROOT=<path>` which specifies where files will be stored/read.
The default root path is `/tmp`. The provider must have read and write access to the root location.
Each actor will store its files under the directory `$ROOT/<actor_id>`.

