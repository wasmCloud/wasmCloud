# Local build

To run this project with your own changes, follow the steps below.

Let's take a quick tour through what we'll need to run the application. We'll need to:

- Build our local version of the application
- Launch a wasmCloud host
- Launch the application and relevant capability providers

## Build

To build the application locally, from the top level we can use `wash`:

```bash
wash build
```

## Start a wasmCloud host

In a separate terminal, start a wasmCloud host:

```console
wash up
```

> [!WARNING]
> This command won't exit, as the host stays running, so you can minimize or create a new terminal.

## Deploy this application

To deploy the application, we can use the WAsmcloud Deployment Manager (WADM), via `wash`:

```console
wash app deploy local.wadm.yaml
```

> [!WARNING]
> Note that we use `local.wadm.yaml` here -- it has a *relative* file reference
> to where the WebAssembly binary (output of `wash build`) should be.

To confirm that the application has been deployed:

```console
wash app list
```

From here, we can follow the usual procedure and try to curl the application:

```console
curl localhost:8080/transform \
    -F "operations[]=filter:fill(yellow)" \
    -F "operations[]=filter:saturation(100)" \
    -F "operations[]=resize:500x500" \
    -F "image=@test/fixtures/terri.png"
```
