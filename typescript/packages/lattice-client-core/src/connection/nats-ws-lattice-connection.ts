import {
  connect,
  type KvEntry,
  type NatsConnection,
  type ConnectionOptions as NatsOptions,
} from 'nats.ws';
import {type JsonValue} from 'type-fest';
import {LatticeConnectionStatus, type LatticeConnection} from '@/connection/lattice-connection';
import {toPromise} from '@/helpers';

type Options = {
  latticeUrl: string;
  natsOptions?: Omit<NatsOptions, 'servers'>;
};

class NatsWsLatticeConnection implements LatticeConnection<Options> {
  #options: Options;
  #connection?: NatsConnection;
  #status: LatticeConnectionStatus = 'initial';

  get options() {
    return this.#options;
  }

  get status() {
    return this.#status;
  }

  constructor(options: Options) {
    this.#options = options;
  }

  setOptions(options: Partial<Options>): void;
  setOptions<Key extends keyof Options = keyof Options>(key: Key, value?: Options[Key]): void;
  setOptions<Key extends keyof Options = keyof Options>(
    key: Key | Partial<Options>,
    value?: Options[Key],
  ): void {
    if (value === undefined) {
      const options = key;
      this.#options = Object.assign(this.#options, options);
    } else if (typeof key === 'string') {
      this.#options[key] = value;
    }
    this.#reconnectIfConnected();
  }

  async connect(): Promise<void> {
    try {
      if (this.#connection) return;

      this.#status = 'pending';

      const connection = await connect({
        servers: this.#options.latticeUrl,
        ...this.#options.natsOptions,
      });

      void connection.closed().then((error) => {
        if (error) {
          this.#connection = undefined;
          this.#status = 'error';
          console.error(`Closed with an error: ${error.message}`);
        }

        this.#connection = undefined;
        this.#status = 'disconnected';
      });

      this.#connection = connection;
      this.#status = 'connected';
    } catch (error) {
      this.#connection = undefined;
      this.#status = 'error';
      throw new Error(
        `Failed to connect to lattice: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    }
  }

  async disconnect(): Promise<void> {
    await this.#connection?.drain();
    this.#status = 'disconnected';
  }

  async request<Response = unknown>(
    subject: string,
    data?: Uint8Array | string | undefined,
  ): Promise<Response> {
    try {
      const connection = await this.#waitForConnection();
      const response = await connection.request(subject, data);
      return response.json<Response>();
    } catch (error) {
      throw new Error(
        `Failed to request ${subject}: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    }
  }

  subscribe<Event = unknown>(subject: string, listenerFunction: (event: Event) => void) {
    const unsubscribe = new AbortController();
    let unsubscribeCalledBeforeConnection = false;

    unsubscribe.signal.addEventListener('abort', () => {
      unsubscribeCalledBeforeConnection = true;
    });

    const result = {
      unsubscribe: () => {
        unsubscribe.abort();
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

      unsubscribe.signal.addEventListener('abort', () => {
        watch.unsubscribe();
      });
    });

    return result;
  }

  async getBucketKeys(bucketName: string): Promise<string[]> {
    try {
      const connection = await this.#waitForConnection();

      const bucket = await connection.jetstream().views.kv(bucketName);
      const keys = await toPromise(await bucket.keys());

      return keys;
    } catch (error) {
      throw new Error(
        `Failed to get keys from bucket ${bucketName}: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    }
  }

  async getBucketEntry<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    bucketName: string,
    key: string,
  ): Promise<ResultType> {
    try {
      const connection = await this.#waitForConnection();

      const bucket = await connection.jetstream().views.kv(bucketName);
      const entry = await bucket.get(key);

      if (entry === null) {
        throw new Error(`Entry with key ${key} not found in bucket ${bucketName}`);
      }

      return entry.json<ResultType>();
    } catch (error) {
      throw new Error(
        `Failed to get entry with key ${key} from bucket ${bucketName}: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    }
  }

  async getBucketEntries<ResultType extends Record<string, JsonValue> = Record<string, JsonValue>>(
    bucketName: string,
  ): Promise<ResultType> {
    try {
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
    } catch (error) {
      throw new Error(
        `Failed to get entries from bucket ${bucketName}: ${error instanceof Error ? error.message : 'Unknown error'}`,
      );
    }
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

  async #waitForConnection(): Promise<NatsConnection> {
    if (this.#status === 'initial')
      throw new Error('Failed to establish connection. Did you call connect()?');

    if (this.#status === 'pending') {
      return new Promise((resolve) => {
        setTimeout(() => {
          resolve(this.#waitForConnection());
        }, 100);
      });
    }

    if (!this.#connection) {
      throw new Error('Connection not established. Did you call connect()?');
    }

    return this.#connection;
  }
}

export {NatsWsLatticeConnection};
