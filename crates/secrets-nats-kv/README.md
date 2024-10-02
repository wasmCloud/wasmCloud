# Secrets NATS KV Backend

This crate implements the wasmCloud secrets backend protocol and supports storing encrypted secrets in a NATS KV bucket.

## Installation

```bash
cargo install --path .
```

## Usage

### Running the secrets backend

Run the binary using the `run` subcommand, supplying xkey private keys to use for encryption and transit. You can generate xkeys using `wash keys gen curve` or the [nk CLI](https://docs.nats.io/using-nats/nats-tools/nk). All other arguments are optional for configuring the topic prefix to listen on, the bucket to store secrets in, etc.

>[!CAUTION]
> ⚠️ These keys are samples to show proper usage and should not be used for your own backend.

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

## Runtime Recommendations

> [!CAUTION]
> This backend is largely intended to provide an example of a secrets backend implementation. It is not recommended for production use, however it may be used in production as long as you are aware of the limitations and risks.

### Key Management

All values in the `WASMCLOUD_SECRETS` bucket are encrypted with a single encryption key (the `ENCRYPTION_XKEY_SEED` environment variable). This key *must* be the same key used to encrypt and decrypt all values in the bucket. If you lose this key, you will not be able to decrypt any of the values stored in the bucket. You *must* back up this key in an external secrets store in order to guarantee that you do not lose this key.

#### Key Rotation

Online key rotation is currently not supported. If you need to rotate the encryption key, you will need to do the following:
* stop all running instances of the secrets-nats-kv backend
* generate a new encryption key
* read every value from the `WASMCLOUD_SECRETS` bucket, decrypt each value with the old key and write them back to the bucket with the new key
* start the new secrets-nats-kv backend instances with the new key

### Resiliency

> [!NOTE]
> You can configure the `WASMCLOUD_SECRETS` bucket by using the `--bucket` flag when running the binary. The default bucket is `WASMCLOUD_SECRETS`, but you will need to adjust the following commands if you change the bucket name.

State for the backend is stored in two buckets: `WASMCLOUD_SECRETS` and `SECRETS-nats-kv_state`. The former stores all secrets while the latter stores all mappings. If you lose the state of the backend, you will lose all secrets and mappings. You *must* back up the state of the backend in order to guarantee that you do not lose any data. You also *must* edit the underlying streams so that they are replicaed to more than one node, which requires running at least a 3-node NATS cluster.

```
nats --creds host.creds stream edit KV_WASMCLOUD_SECRETS --replicas 3
nats --creds host.creds stream edit KV_SECRETS-nats-kv_state --replicas 3
```

You may want to adjust the replicas to 5 instead of 3 depending on your risk tolerance. You will need at least 3 or 5 members of the NATS cluster in order for these commands to succeed.


You should also run more than one instance of the secrets-nats-kv backend.
