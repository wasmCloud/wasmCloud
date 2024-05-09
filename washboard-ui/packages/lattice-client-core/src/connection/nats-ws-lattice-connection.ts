import {connect, type KvEntry, type NatsConnection} from 'nats.ws';
import {type JsonValue} from 'type-fest';
import {toPromise} from '../helpers';
import {type LatticeConnection} from './lattice-connection';

type NatsWsLatticeConnectionOptions = {
  retryCount: number;
  latticeUrl: string;
};

class NatsWsLatticeConnection implements LatticeConnection {
  readonly #options: NatsWsLatticeConnectionOptions;
  #connection?: NatsConnection;
  #status: typeof LatticeConnection.prototype.status = 'pending';

  get status() {
    return this.#status;
  }

  constructor(options: NatsWsLatticeConnectionOptions) {
    this.#options = options;
    void this.connect();
  }

  setLatticeUrl(url: string): void {
    this.#options.latticeUrl = url;
    this.#reconnectIfConnected();
  }

  setRetryCount(count: number): void {
    this.#options.retryCount = count;
    this.#reconnectIfConnected();
  }

  async connect(): Promise<void> {
    if (this.#connection) return;

    this.#status = 'pending';

    this.#connection = await connect({
      servers: this.#options.latticeUrl,
    });
    void this.#connection.closed().then((error) => {
      if (error) {
        this.#status = 'error';
        console.error(`closed with an error: ${error.message}`);
      }

      this.#status = 'disconnected';
    });
  }

  async disconnect(): Promise<void> {
    await this.#connection?.drain();
    this.#status = 'disconnected';
  }

  async request<Response = unknown>(
    subject: string,
    data?: Uint8Array | string | undefined,
  ): Promise<Response> {
    const connection = await this.#waitForConnection();
    const response = await connection.request(subject, data);
    return response.json<Response>();
  }

  subscribe<Event = unknown>(subject: string, listenerFunction: (event: Event) => void) {
    let unsubscribeCalledBeforeConnection = false;

    const result = {
      unsubscribe() {
        unsubscribeCalledBeforeConnection = true;
      },
    };

    void this.#waitForConnection().then((connection) => {
      const watch = connection.subscribe(subject, {
        callback(error, message) {
          if (error) throw error;
          const parsedEvent = message.json<Event>();
          listenerFunction(parsedEvent);
        },
      });

      if (unsubscribeCalledBeforeConnection) {
        watch.unsubscribe();
        return;
      }

      result.unsubscribe = () => {
        watch.unsubscribe();
      };
    });

    return result;
  }

  async getBucketKeys(bucketName: string): Promise<string[]> {
    const connection = await this.#waitForConnection();

    const bucket = await connection.jetstream().views.kv(bucketName);
    const keys = await toPromise(await bucket.keys());

    return keys;
  }

  async getBucketEntry<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    bucketName: string,
    key: string,
  ): Promise<ResultType> {
    const connection = await this.#waitForConnection();

    const bucket = await connection.jetstream().views.kv(bucketName);
    const entry = await bucket.get(key);

    if (entry === null) {
      throw new Error(`Entry with key ${key} not found in bucket ${bucketName}`);
    }

    return entry.json<ResultType>();
  }

  async getBucketEntries<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    bucketName: string,
  ): Promise<ResultType> {
    const connection = await this.#waitForConnection();

    const bucket = await connection.jetstream().views.kv(bucketName);
    const keys = await toPromise(await bucket.keys());

    const maybeEntries = await Promise.all(keys.map(async (key) => bucket.get(key)));

    const entries = maybeEntries.reduce((entries: Record<string, JsonValue>, entry) => {
      if (entry === null) return entries;

      const {key} = entry;
      const value = entry.json<JsonValue>();
      entries[key] = value;

      return entries;
    }, {}) as ResultType;

    return entries;
  }

  watchBucket(
    bucketName: string,
    listenerFunction: (entry: KvEntry) => void,
    options?: {key?: string},
  ): void {
    (async () => {
      const connection = await this.#waitForConnection();
      const bucket = await connection.jetstream().views.kv(bucketName);

      const changes = await bucket.watch({key: options?.key});
      for await (const entry of changes) {
        listenerFunction(entry);
      }
    })();
  }

  readonly #reconnectIfConnected = () => {
    if (this.#status === 'connected' || this.#status === 'pending') {
      void (async () => {
        await this.disconnect();
        await this.connect();
      })();
    }
  };

  readonly #waitForConnection = async (count = 0): Promise<NatsConnection> =>
    new Promise((resolve, reject) => {
      if (count >= this.#options.retryCount) {
        reject(new Error('Could not connect to lattice'));
        return;
      }

      try {
        if (this.#connection) {
          resolve(this.#connection);
        } else {
          setTimeout(() => {
            resolve(this.#waitForConnection(count + 1));
          }, 100);
        }
      } catch (error) {
        reject(error as Error);
      }
    });
}

export {NatsWsLatticeConnection};
