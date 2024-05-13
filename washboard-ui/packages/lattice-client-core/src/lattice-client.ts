import {type LatticeEvent} from '@/cloud-events';
import {type LatticeConnection} from '@/connection/lattice-connection';
import {NatsWsLatticeConnection} from '@/connection/nats-ws-lattice-connection';
import {ApplicationsController} from '@/controllers/applications';
import {ComponentController} from '@/controllers/components';
import {ConfigsController} from '@/controllers/configs';
import {HostsController} from '@/controllers/hosts';
import {LinksController} from '@/controllers/links';
import {ProvidersController} from '@/controllers/providers';

type LatticeClientConfig = {
  latticeUrl: string;
  retryCount?: number;
  latticeId?: string;
  ctlTopicPrefix?: string;
  wadmTopicPrefix?: string;
};

export type LatticeClientOptions = {
  config: LatticeClientConfig;
  connection?: LatticeConnection;
  autoConnect?: boolean;
};

export const defaultConfig: Required<Omit<LatticeClientConfig, 'latticeUrl' | 'connection'>> = {
  retryCount: 10,
  latticeId: 'default',
  ctlTopicPrefix: 'wasmbus',
  wadmTopicPrefix: 'wadm',
};

export class LatticeClient {
  readonly #connection: LatticeConnection;
  #config: Omit<Required<LatticeClientConfig>, 'connection'>;

  get #latticeId(): string {
    return this.#config.latticeId;
  }

  get #ctlTopicPrefix(): string {
    return this.#config.ctlTopicPrefix;
  }

  get #ctlTopic(): string {
    return `${this.#ctlTopicPrefix}.ctl.v1.${this.#latticeId}`;
  }

  get #wadmTopic(): string {
    return `${this.#config.wadmTopicPrefix}.api.${this.#latticeId}`;
  }

  get connection() {
    return this.#connection;
  }

  /**
   * Methods and properties to interact with this LatticeClient instance
   */
  get instance() {
    return {
      /** The configuration for the client */
      config: {
        ...this.#config,
        ctlTopic: this.#ctlTopic,
        wadmTopic: this.#wadmTopic,
      },
      /** Send a request on the connected lattice */
      request: this.#request.bind(this),
      /** subscribe to a specific topic on the connected lattice */
      subscribe: this.#subscribe.bind(this),
      /** Connect to the lattice */
      connect: this.#connect.bind(this),
      /** Disconnect from the lattice */
      disconnect: this.#disconnect.bind(this),
      /** Disconnect and reconnect to the lattice */
      reconnect: this.#reconnect.bind(this),
      /** Update the client with a partial configuration */
      setPartialConfig: this.#setPartialConfig.bind(this),
    };
  }

  /**
   * Methods and properties to interact with the lattice hosts
   */
  get hosts() {
    return new HostsController(this);
  }

  /**
   * Methods to interact with components
   */
  get components() {
    return new ComponentController(this);
  }

  /**
   * Methods to interact with providers
   */
  get providers() {
    return new ProvidersController(this);
  }

  /**
   * Methods to interact with links
   */
  get links() {
    return new LinksController(this);
  }

  /**
   * Methods to interact with configs
   */
  get configs() {
    return new ConfigsController(this);
  }

  /**
   * Methods to interact with Wadm Applications
   */
  get applications() {
    return new ApplicationsController(this);
  }

  /**
   * Create a new LatticeClient
   * @param options.config the configuration for the client. This will be merged with the default
   * configuration and can be changed later with `client.instance.setPartialConfig`
   * @param options.connection (optional) the connection to use for the client. If not provided, a
   * new connection will be created with the latticeUrl from the config
   */
  constructor({config, connection, autoConnect = true}: LatticeClientOptions) {
    this.#config = {
      ...defaultConfig,
      ...config,
    };

    this.#connection = connection ?? new NatsWsLatticeConnection(this.#config);

    if (autoConnect !== false) {
      // try and connect, but don't throw an error if it fails. The connection will be in an error state accessible
      // through the `client.connection.status` property
      this.#connect().catch(() => {
        console.info('Failed to connect to lattice on creation');
      });
    }
  }

  /**
   * Update the client with a partial configuration. Existing keys that are not provided will remain the same.
   * @param newConfig partial configuration to update the client with
   */
  #setPartialConfig(newConfig: Partial<LatticeClientConfig>) {
    this.#config = {
      ...this.#config,
      ...newConfig,
    };

    if (newConfig.latticeUrl) {
      this.#connection.setLatticeUrl(newConfig.latticeUrl);
    }

    if (newConfig.retryCount) {
      this.#connection.setRetryCount(newConfig.retryCount);
    }

    // try and reconnect with the new configuration
    if (this.#connection.status === 'connected') {
      this.#reconnect().catch(() => null);
    }
  }

  /**
   * Send a request to the lattice
   * @param subject the nats subject to send the request to
   * @param data (optional) the data to send with the request
   * @returns the response from the lattice without any processing
   */
  async #request<Response = unknown>(subject: string, data?: Uint8Array | string) {
    if (this.#connection === undefined) {
      throw new Error('Connection not initialized');
    }

    return this.#connection.request<Response>(subject, data);
  }

  /**
   * subscribe to a latticeTopic
   * @param subject latticeTopic to subscribe to
   * @param callback callback to invoke when an event is received
   */
  #subscribe<Event extends LatticeEvent>(subject: string, callback: (event: Event) => void) {
    if (this.#connection === undefined) {
      throw new Error('Connection not initialized');
    }

    return this.#connection.subscribe(subject, callback);
  }

  /**
   * Connect to the lattice
   */
  readonly #connect = async (): Promise<void> => {
    await this.#connection?.connect();
  };

  /**
   * Disconnect from the lattice
   */
  readonly #disconnect = async (): Promise<void> => {
    await this.#connection?.disconnect();
  };

  /**
   * Convenience method to disconnect and reconnect to the lattice
   */
  readonly #reconnect = async (): Promise<void> => {
    await this.#disconnect();
    await this.#connect();
  };
}
