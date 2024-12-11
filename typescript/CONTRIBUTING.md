# Contributing

## How-To

### Contribute code

1. Install the dependencies.
2. Start the wasmCloud host using the `wash` CLI. Read more about it [here](#start-the-wasmcloud-host-using-wash-cli).
   1. Ensure the NATS service is running with the websocket listener enabled.
3. Start a local frontend development server. Read more about it [here](#start-a-local-ui-development-server).
4. Make changes to the UI.
5. Commit your changes.
6. Open a pull request.
7. Wait for the CI to pass.
8. Wait for a maintainer to review your changes.
9. Wait for a maintainer to merge your changes.
10. 🚀 🏁 Done

### Install dependencies

Enable `corepack` and install `yarn` using the following commands:

```bash
# optional, run only if you haven't enabled corepack before
corepack enable
yarn install
```
### Building ts files

Run the following command to build typescript files in @wasmcloud/.. workspace package
to avoid the following error: 

`Unable to resolve path to module '@wasmcloud/lattice-client-react'  import/no-unresolved`

```bash
yarn run turbo:build
```
### Start a local UI development server

Run the following command to start a local frontend development server:

```bash
yarn run dev
```

### Start the wasmCloud Host using wash CLI

Run the following command to start the wasmCloud host using the wash CLI:

```bash
wash up
```

### Explanations

#### NATS

`wasmcloud` uses [NATS](https://nats.io/) as its message broker. The `wash` CLI can be used to start a local NATS
or connect to an existing NATS server.

The Washboard UI connects to a NATS server at [ws://localhost:4223 by default][0], although this can be overridden via
the UI.

You can spawn the NATS server with a wasmCloud Host and wadm with the `wash` cli tool. See the documentation for [installation instructions][1].

```bash
wash up
```

You can change the NATS websocket port using the
`--nats-websocket-port` flag or `NATS_WEBSOCKET_PORT` environment variable. Note that you must stop the NATS Server to make this change. For example:

```bash
# 1. Make sure NATS is stopped
wash down

# 2. Start NATS with the new port
wash up --nats-websocket-port 4001
# or
NATS_WEBSOCKET_PORT=4001 wash up
```

Otherwise, verify the port you are using to connect to the NATS server. Visit [NATS Websocket Configuration][2] for more
information.

[0]: https://github.com/wasmCloud/wasmCloud/blob/28699dc8e891df34888935e8ace31c718da9f590/washboard-ui/src/lattice/lattice-service.ts#L70
[1]: https://wasmcloud.com/docs/installation
[2]: https://docs.nats.io/running-a-nats-service/configuration/websocket/websocket_conf
