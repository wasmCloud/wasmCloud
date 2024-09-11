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

abstract class LatticeConnection {
  abstract status: LatticeConnectionStatus;

  abstract setLatticeUrl(url: string): void;

  abstract setRetryCount(count: number): void;

  abstract connect(): Promise<void>;

  abstract disconnect(): Promise<void>;

  abstract request<Response = unknown>(
    subject: string,
    data?: Uint8Array | string | undefined,
  ): Promise<Response>;

  abstract subscribe<Event = unknown>(
    subject: string,
    callback: (event: Event) => void,
  ): {
    unsubscribe: () => void;
  };

  abstract getBucketKeys(bucketName: string): Promise<string[]>;

  abstract getBucketEntry<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    subject: string,
    key: string,
  ): Promise<ResultType>;

  abstract getBucketEntries<
    ResultType extends Record<string, JsonValue> = Record<string, JsonValue>,
  >(bucketName: string): Promise<ResultType>;

  abstract watchBucket<BucketContents>(
    bucketName: string,
    callback: (entry: BucketResult<BucketContents>) => void,
    options?: {key?: string},
  ): void;
}

export {LatticeConnection};
