import {produce} from 'immer';
import {NatsConnection, connect} from 'nats.ws';
import {BehaviorSubject, Observable, map, merge, tap} from 'rxjs';
import {CloudEvent, LinkResponse, WadmComponent, WadmConfig, WadmHost, WadmLink, WadmProvider} from './types';

export type LatticeCache = {
  hosts: Record<string, WadmHost>;
  components: Record<string, WadmComponent>;
  providers: Record<string, WadmProvider>;
  links: WadmLink[];
  configs: Record<string, WadmConfig>;
};

export type LatticeClientConfig = {
  latticeUrl: string;
  retryCount: number;
  latticeId: string;
  ctlTopicPrefix: string;
};

export type LatticeClientOptions = {
  config: Partial<LatticeClientConfig> & Required<Pick<LatticeClientConfig, 'latticeUrl'>>;
};

export const defaultConfig: Required<Omit<LatticeClientConfig, 'latticeUrl'>> = {
  retryCount: 10,
  latticeId: 'default',
  ctlTopicPrefix: 'wasmbus',
};

export class LatticeClient {
  config$: BehaviorSubject<LatticeClientConfig>;

  #connection?: NatsConnection;
  #linkState$: BehaviorSubject<Pick<LatticeCache, 'links'>>;
  #wadmState$: BehaviorSubject<Partial<Pick<LatticeCache, 'components' | 'providers' | 'hosts'>>>;
  #configState$: BehaviorSubject<Pick<LatticeCache, 'configs'>>;

  constructor({config}: LatticeClientOptions) {
    this.#linkState$ = new BehaviorSubject<Pick<LatticeCache, 'links'>>({links: []});
    this.#configState$ = new BehaviorSubject<Pick<LatticeCache, 'configs'>>({configs: {}});
    this.#wadmState$ = new BehaviorSubject<Partial<Pick<LatticeCache, 'components' | 'providers' | 'hosts'>>>({ components: {}, providers: {}, hosts: {}});
    this.config$ = new BehaviorSubject({
      ...defaultConfig,
      ...config,
    });
  }

  get config(): Required<LatticeClientConfig> {
    return this.config$.value;
  }

  setPartialConfig(newConfig: Partial<LatticeClientConfig>) {
    const oldConfig = this.config$.value;
    this.config$.next({
      ...oldConfig,
      ...newConfig,
    });
    this.reconnect();
  }

  get #latticeId(): string {
    return this.config.latticeId;
  }

  get #ctlTopicPrefix(): string {
    return this.config.ctlTopicPrefix;
  }

  get #ctlTopic(): string {
    return `${this.#ctlTopicPrefix}.ctl.v1.${this.#latticeId}`;
  }

  connect = async (): Promise<void> => {
    this.#connection = await connect({
      servers: this.config.latticeUrl,
    });
    this.#connection.closed().then((error) => {
      if (error) {
        console.error(`closed with an error: ${error.message}`);
      }
    });
    this.#subscribeToWadmState();
    this.#subscribeToLinks();
    this.#subscribeToConfigs();
  };

  disconnect = async (): Promise<void> => {
    await this.#connection?.drain().catch(() => null);
  };

  reconnect = async (): Promise<void> => {
    await this.disconnect();
    await this.connect();
  };

  getLatticeCache$ = (): Observable<LatticeCache> => {
    const subject = new BehaviorSubject<LatticeCache>({
      hosts: {},
      components: {},
      providers: {},
      links: [],
      configs: {},
    });

    // join wadmState and #linkState into a single observable
    merge(this.#wadmState$, this.#linkState$, this.#configState$)
      .pipe(
        // merge the new event into the existing state
        map((event) =>
          produce(subject.getValue(), (draft) => ({
            ...draft,
            ...event,
          })),
        ),
        // update the subject with the new state
        tap((state) => subject.next(state)),
      )
      .subscribe();

    return subject;
  };

  #subscribeToLinks = (): void => {
    (async (): Promise<void> => {
      const LINK_TOPIC = `${this.#ctlTopic}.link.get`;
      const connection = await this.#waitForConnection();
      const json = await (await connection.request(LINK_TOPIC)).json<LinkResponse>()
      if (json.success) {
        this.#linkState$.next({links: json.response});
      } else {
        throw new Error(json.message);
      }

      // TODO: ideally we'll want to subscribe to the individual event topics but for now, that'll do 🐷
      const watch = await connection.subscribe(`wasmbus.evt.${this.#latticeId}.*`);
      for await (const event of watch) {
        const parsedEvent = event.json<CloudEvent>();
        switch (parsedEvent.type) {
          case 'com.wasmcloud.lattice.linkdef_set':
          case 'com.wasmcloud.lattice.linkdef_deleted': {
            // Just refresh the whole list instead of trying to figure out which one changed
            const message = await connection.request(LINK_TOPIC);
            this.#linkState$.next(message.json<{links: WadmLink[]}>());
          }
        }
      }
      this.#linkState$.complete();
    })();
  };

  #subscribeToConfigs = (): void => {
    (async (): Promise<void> => {
      const connection = await this.#waitForConnection();
      const configs = await connection.jetstream().views.kv('CONFIGDATA_default');
      const watch = await configs.watch();
      for await (const event of watch) {
        const existingConfigs = this.#configState$.getValue().configs
        const newConfigs = { [event.key]: { name: event.key, entries: event.json<{[key: string]: string}>() }};
        this.#configState$.next({configs: {...existingConfigs, ...newConfigs}});
      }
      this.#configState$.complete();
    })();
  }

  #subscribeToWadmState = (): void => {
    (async (): Promise<void> => {
      const connection = await this.#waitForConnection();
      const wadm = await connection.jetstream().views.kv('wadm_state');
      const watch = await wadm.watch();
      for await (const event of watch) {
        switch (event.key) {
          case 'host_default': {
            this.#wadmState$.next({hosts: event.json() as Record<string, WadmHost>});
            break;
          }
          case 'component_default': {
            this.#wadmState$.next({components: event.json() as Record<string, WadmComponent>});
            break;
          }
          case 'provider_default': {
            this.#wadmState$.next({providers: event.json() as Record<string, WadmProvider>});
            break;
          }
        }
      }
      this.#wadmState$.complete();
    })();
  };

  #waitForConnection = (count = 0): Promise<NatsConnection> => {
    return new Promise((resolve, reject) => {
      if (count >= this.config$.value.retryCount) {
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
        reject(error);
      }
    });
  };
}
