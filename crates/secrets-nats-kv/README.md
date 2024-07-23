# Secrets NATS KV Backend

This crate implements the wasmCloud secrets backend protocol and supports storing encrypted secrets in a NATS KV bucket.

## Installation

```bash
cargo install --path .
```

## Usage

### Running the secrets backend

Run the binary using the `run` subcommand, supplying xkey private keys to use for encryption and transit. You can generate xkeys using `wash keys gen x25519` or the [nk CLI](https://docs.nats.io/using-nats/nats-tools/nk). All other arguments are optional for configuring the topic prefix to listen on, the bucket to store secrets in, etc.

⚠️ These keys are samples to show proper usage and should not be used for your own backend.

```bash
nats-server -js &
TRANSIT_XKEY_SEED=SXAC35QF3FMZXS2KGYXGF2DN45JSSDYQM3CQMWAZJW5NMA7Y7BCMVSWL4A \
    ENCRYPTION_XKEY_SEED=SXAIPHCTMQ5M7KWEVKBWZ37ZVQVMCRJGKSIXCNMKDHTH4YPPJTIOOVV4WQ \
    secrets-nats-kv run
```

### Managing secrets

This binary is also a CLI that allows you to manage secrets in a running NATS KV secrets backend instance. For all of the below commands, ensure that you are currently running the binary and it's accessible over NATS.

#### Add a secret

To add a string secret:

```bash
TRANSIT_XKEY_SEED=SXAC35QF3FMZXS2KGYXGF2DN45JSSDYQM3CQMWAZJW5NMA7Y7BCMVSWL4A \
    secrets-nats-kv put secret-foo --string sup3rs3cr3t
```

Keep in mind that shell history is stored in plaintext on your device, and you may want to consider using the `SECRET_STRING_VALUE` environment variable instead of using the flag.

To upload a file as bytes as a binary secret:

```bash
TRANSIT_XKEY_SEED=SXAC35QF3FMZXS2KGYXGF2DN45JSSDYQM3CQMWAZJW5NMA7Y7BCMVSWL4A \
    secrets-nats-kv put secret-foo --binary ./path/to/secret.bin
```

#### Allow a component or provider to access a secret

You can find the public key of any component or provider built using `wash build` by running `wash inspect <reference>`.

```bash
➜ wash inspect ghcr.io/wasmcloud/components/dog-fetcher-rust:0.1.1


                           dog-fetcher - Component
  Account         ACZBUFUCUBYM3EGEIV6C2TSC5AWG52WI5TQM2CXDIZRU2R7D3IJYXUZQ
  Component       MAVCGEGKMVT5UCIDSHJO25VHD2VDNDRA3LIHYH2TPIUQS7JCMS472AFJ
  Expires                                                            never
  Can Be Used                                                  immediately
  Version                                                        0.1.0 (0)
  Embedded WIT                                                        true
```

This command will allow the dog-fetcher component to access the secret `secret-foo`. You can specify the `--secret` flag multiple times to allow for accessing multiple secrets.

```bash
secrets-nats-kv add-mapping MAVCGEGKMVT5UCIDSHJO25VHD2VDNDRA3LIHYH2TPIUQS7JCMS472AFJ --secret secret-foo
```

#### Disallow a component or provider to access a secret

All secrets are accessed using an allow-list of mappings. You can remove a mapping in the same way you specified it.

```bash
secrets-nats-kv remove-mapping MAVCGEGKMVT5UCIDSHJO25VHD2VDNDRA3LIHYH2TPIUQS7JCMS472AFJ --secret secret-foo
```
