export namespace WasmcloudSecretsStore {
  /**
   * Gets a single opaque secrets value set at the given key if it exists
   */
  export function get(key: string): Secret;
  export { Secret };
}
/**
 * An error type that encapsulates the different errors that can occur fetching secrets
 */
export type SecretsError = SecretsErrorUpstream | SecretsErrorIo | SecretsErrorNotFound;
/**
 * This indicates an error from an "upstream" secrets source.
 * As this could be almost _anything_ (such as Vault, Kubernetes Secrets, KeyValue buckets, etc),
 * the error message is a string.
 */
export interface SecretsErrorUpstream {
  tag: 'upstream',
  val: string,
}
/**
 * This indicates an error from an I/O operation.
 * As this could be almost _anything_ (such as a file read, network connection, etc),
 * the error message is a string.
 * Depending on how this ends up being consumed,
 * we may consider moving this to use the `wasi:io/error` type instead.
 * For simplicity right now in supporting multiple implementations, it is being left as a string.
 */
export interface SecretsErrorIo {
  tag: 'io',
  val: string,
}
/**
 * This indicates that the secret was not found. Generally "not found" errors will
 * be handled by the upstream secrets backend, but there are cases where the host
 * may need to return this error.
 */
export interface SecretsErrorNotFound {
  tag: 'not-found',
}
/**
 * A secret value can be either a string or a byte array, which lets you
 * store binary data as a secret.
 */
export type SecretValue = SecretValueString | SecretValueBytes;
/**
 * A string value
 */
export interface SecretValueString {
  tag: 'string',
  val: string,
}
/**
 * A byte array value
 */
export interface SecretValueBytes {
  tag: 'bytes',
  val: Uint8Array,
}

export class Secret {
}
