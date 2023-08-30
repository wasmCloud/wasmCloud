import {produce} from 'immer';
import {NatsConnection, connect} from 'nats.ws';
import {BehaviorSubject, Observable, Subject, map, merge, tap} from 'rxjs';
import {CloudEvent} from './cloud-event.type';

export interface LatticeCache {
  hosts: Record<string, WadmHost>;
  actors: Record<string, WadmActor>;
  providers: Record<string, WadmProvider>;
  links: WadmLink[];
}

export interface WadmActor {
  id: string;
  name: string;
  capabilities: string[];
  issuer: string;
  reference: string;
  instances: Record<
    string,
    {
      instance_id: string;
      annotations: Record<string, string>;
    }[]
  >;
}

export interface WadmProvider {
  id: string;
  name: string;
  issuer: string;
  contract_id: string;
  reference: string;
  link_name: string;
  hosts: Record<string, string>;
}

export interface WadmLink {
  actor_id: string;
  contract_id: string;
  link_name: string;
  public_key: string;
  provider_id: string;
}

export interface WadmHost {
  friendly_name: string;
  id: string;
  labels: Record<string, string>;
  annotations: Record<string, string>;
  last_seen: string;
  actors: Record<string, number>;
  providers: {
    contract_id: 'wasmcloud:httpserver';
    link_name: 'default';
    public_key: 'VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M';
    annotations: Record<string, string>;
  }[];
  uptime_seconds: number;
  version: string;
}

class LatticeService {
  private static instance: LatticeService;
  private static readonly RETRY_COUNT = 10;

  private connection?: NatsConnection;

  private config = {
    latticeUrl: import.meta.env.VITE_NATS_WEBSOCKET_URL || 'ws://localhost:4001',
  };

  private constructor() {
    this.configureConnection();
  }

  public set latticeUrl(url: string) {
    this.config.latticeUrl = url;
    this.configureConnection();
  }
  public get latticeUrl(): string {
    return this.config.latticeUrl;
  }

  public static getInstance(): LatticeService {
    if (!LatticeService.instance) {
      LatticeService.instance = new LatticeService();
    }
    return LatticeService.instance;
  }

  public getLatticeCache$(): Observable<LatticeCache> {
    const subject = new BehaviorSubject<LatticeCache>({
      hosts: {},
      actors: {},
      providers: {},
      links: [],
    });

    // join wadmState and linkState into a single observable
    merge(this.wadmState$(), this.linkState$())
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
  }

  public linkState$(): Subject<Pick<LatticeCache, 'links'>> {
    const subject = new Subject<Pick<LatticeCache, 'links'>>();

    (async (): Promise<void> => {
      const connection = await this.waitForConnection();
      const message = await connection.request('wasmbus.ctl.default.get.links');
      subject.next(message.json<{links: WadmLink[]}>());

      const watch = await connection.subscribe('wasmbus.evt.default');
      for await (const event of watch) {
        const parsedEvent = event.json<CloudEvent>();
        switch (parsedEvent.type) {
          case 'com.wasmcloud.lattice.linkdef_set':
          case 'com.wasmcloud.lattice.linkdef_deleted': {
            // Just refresh the whole list instead of trying to figure out which one changed
            const message = await connection.request('wasmbus.ctl.default.get.links');
            subject.next(message.json<{links: WadmLink[]}>());
          }
        }
      }
      subject.complete();
    })();

    return subject;
  }

  public wadmState$(): Subject<Partial<LatticeCache>> {
    const subject = new Subject<Partial<LatticeCache>>();

    (async (): Promise<void> => {
      const connection = await this.waitForConnection();
      const wadm = await connection.jetstream().views.kv('wadm_state');
      const watch = await wadm.watch();
      for await (const event of watch) {
        switch (event.key) {
          case 'host_default': {
            subject.next({hosts: event.json() as Record<string, WadmHost>});
            break;
          }
          case 'actor_default': {
            subject.next({actors: event.json() as Record<string, WadmActor>});
            break;
          }
          case 'provider_default': {
            subject.next({providers: event.json() as Record<string, WadmProvider>});
            break;
          }
        }
      }
      subject.complete();
    })();

    return subject;
  }

  private async configureConnection(): Promise<void> {
    await this.connection?.drain().catch(() => null);
    this.connection = await connect({
      servers: this.latticeUrl,
    });
    this.connection.closed().then((error) => {
      if (error) {
        console.error(`closed with an error: ${error.message}`);
      }
    });
  }

  public async testConnection(url: string): Promise<boolean> {
    try {
      const connection = await connect({
        servers: url,
      });

      await connection.close();
      return true;
    } catch {
      return false;
    }
  }

  private waitForConnection(count = 0): Promise<NatsConnection> {
    return new Promise((resolve, reject) => {
      if (count >= LatticeService.RETRY_COUNT) {
        reject(new Error('Could not connect to lattice'));
        return;
      }

      try {
        if (this.connection) {
          resolve(this.connection);
        } else {
          setTimeout(() => {
            resolve(this.waitForConnection(count + 1));
          }, 100);
        }
      } catch (error) {
        reject(error);
      }
    });
  }
}

export default LatticeService;
