<h1 align="center">📬 email-outgoing-provider</h1>

This repository contains the code for a [wasmCloud capability provider][wasmcloud-docs-providers] that provides outgoing email sending, written in [Rust][rust]. 

[rust]: https://rust-lang.org
[wasmcloud]: https://wasmcloud.com/docs
[wasmcloud-docs-providers]: https://wasmcloud.com/docs/concepts/providers

## 🧱 Dependencies

- [Wasmcloud Shell (`wash`, >= v0.27.0)][wash] (`cargo install wash-cli`)
- [`just`][just] (`cargo install just`)
- [`docker`][docker] for running the local testing SMTP server

> [!WARNING]
> Ensure commands like `docker ps` or `docker info` work *before* running the Quickstart below!

[wash]: https://wasmcloud.com/docs/installation
[just]: https://github.com/casey/just
[docker]: https://docs.docker.com

## 👟 Quickstart

To get started quickly, use the `Justfile` in the repository:

```console
just deploy-demo
```

> [!NOTE]
> You can list all the runnable targets in the Justfile by running `just` or `just --list`

The `deploy-demo` target will:

- Start a local SMTP server with `docker` ([Mailcatcher][mailcatcher] via [`dockage/mailcatcher`](https://hub.docker.com/r/dockage/mailcatcher))
- Start a wasmCloud host with `wash` (i.e. running `wash up --detached`)
- Build the outgoing email provider (`email-outgoing-provider`)
- Build the example email component (`email-hello-world`)
- Set up configuration, provider, component and links 

Once `deploy-demo` has finished, your single-host lattice should be running everything it needs -- you can inspect the lattice with the usual tooling (i.e. `wash`):

```console
wash get inventory
```

**Once you're ready, you can invoke an example email send:**

```console
just send-demo-email
``` 

You can check out your Mailcatcher instance's email at `http://localhost:1080` -- you should see a hello world email there.

> [!WARNING]
> Note that any emails that you send to Mailcatcher will be deleted when it's restarted (undeploying/redeploying the demo)!

The Justfile target boils down to the following `wash call` invocation:

```console
wash call email-hello-world "examples:email-hello-world/invoke.call"
```

To undeploy the demo:

```console
just undeploy-demo
```

[mailcatcher]: https://mailcatcher.me/
