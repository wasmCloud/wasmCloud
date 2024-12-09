export namespace WasiClocksMonotonicClock {
  export function now(): Instant;
  export function resolution(): Duration;
  export function subscribeInstant(when: Instant): Pollable;
  export function subscribeDuration(when: Duration): Pollable;
}
import type { Pollable } from './wasi-io-poll.js';
export { Pollable };
export type Instant = bigint;
export type Duration = bigint;
