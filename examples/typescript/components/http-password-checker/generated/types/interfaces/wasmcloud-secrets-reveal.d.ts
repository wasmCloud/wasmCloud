export namespace WasmcloudSecretsReveal {
  /**
   * Reveals the value of a secret to the caller.
   * This lets you easily audit your code to discover where secrets are being used.
   */
  export function reveal(s: Secret): SecretValue;
}
import type { Secret } from './wasmcloud-secrets-store.js';
export { Secret };
import type { SecretValue } from './wasmcloud-secrets-store.js';
export { SecretValue };
