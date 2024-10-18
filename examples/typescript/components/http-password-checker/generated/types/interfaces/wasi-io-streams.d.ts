export namespace WasiIoStreams {
  export { InputStream };
  export { OutputStream };
}
import type { Error } from './wasi-io-error.js';
export { Error };
import type { Pollable } from './wasi-io-poll.js';
export { Pollable };
export type StreamError = StreamErrorLastOperationFailed | StreamErrorClosed;
export interface StreamErrorLastOperationFailed {
  tag: 'last-operation-failed',
  val: Error,
}
export interface StreamErrorClosed {
  tag: 'closed',
}

export class InputStream {
  read(len: bigint): Uint8Array;
  blockingRead(len: bigint): Uint8Array;
  skip(len: bigint): bigint;
  blockingSkip(len: bigint): bigint;
  subscribe(): Pollable;
}

export class OutputStream {
  checkWrite(): bigint;
  write(contents: Uint8Array): void;
  blockingWriteAndFlush(contents: Uint8Array): void;
  flush(): void;
  blockingFlush(): void;
  subscribe(): Pollable;
  writeZeroes(len: bigint): void;
  blockingWriteZeroesAndFlush(len: bigint): void;
  splice(src: InputStream, len: bigint): bigint;
  blockingSplice(src: InputStream, len: bigint): bigint;
}
