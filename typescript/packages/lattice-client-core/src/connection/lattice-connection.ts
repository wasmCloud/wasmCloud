import {type KvEntry} from 'nats.ws';
import {type JsonValue} from 'type-fest';

type BucketResult<ResultType> = KvEntry & {
  json(): ResultType;
};

export type LatticeConnectionStatus =
  | 'initial'
  | 'connected'
  | 'pending'
  | 'error'
  | 'disconnected';

export type LatticeConnection<Options = unknown> = {
  options: Options;

  setOptions(options: Options): void;
  setOptions(options: Partial<Options>): void;
  setOptions<Key extends keyof Options>(key: Key, value: Options[Key]): void;

  status: LatticeConnectionStatus;

  connect(): Promise<void>;

  disconnect(): Promise<void>;

  request<Response = unknown>(
    subject: string,
    data?: Uint8Array | string | undefined,
  ): Promise<Response>;

  subscribe<Event = unknown>(
    subject: string,
    callback: (event: Event) => void,
  ): {
    unsubscribe: () => void;
  };

  getBucketKeys(bucketName: string): Promise<string[]>;

  getBucketEntry<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    subject: string,
    key: string,
  ): Promise<ResultType>;

  getBucketEntries<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    bucketName: string,
  ): Promise<ResultType>;

  watchBucket<BucketContents>(
    bucketName: string,
    callback: (entry: BucketResult<BucketContents>) => void,
    options?: {key?: string},
  ): void;
};
