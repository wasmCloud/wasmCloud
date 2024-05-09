import {type LatticeClient, getCombinedInventoryFromHosts} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {useLatticeClient} from './context/use-lattice-client';

function useLatticeData() {
  const client = useLatticeClient();
  const [state, handleStateUpdate] = React.useState<LatticeData>({
    hosts: {},
    components: {},
    providers: {},
    links: [],
    configs: {},
  });

  const fetchState = React.useMemo(() => createStateFetcher(client), [client]);

  React.useEffect(() => {
    void fetchState().then((state) => {
      handleStateUpdate(state);
    });
  }, [fetchState]);

  return state;
}

async function fetchInventories(client: LatticeClient) {
  const result = await client.hosts.list({expand: true});
  const hostsList = result.response;
  const hosts = Object.fromEntries(hostsList.map((host) => [host.host_id, host]));
  const {components, providers} = getCombinedInventoryFromHosts(hosts);
  return {hosts, components, providers};
}

async function fetchLinks(client: LatticeClient) {
  const result = await client.links.list();

  return result.response;
}

async function fetchConfigs(client: LatticeClient) {
  const result = await client.configs.list({expand: true});
  const configs = Object.fromEntries(result.response.map((config) => [config.key, config]));
  return configs;
}

type LatticeData =
  ReturnType<typeof createStateFetcher> extends (client: LatticeClient) => Promise<infer T>
    ? T
    : never;

function createStateFetcher(client: LatticeClient) {
  return async () => {
    const [{hosts, components, providers}, links, configs] = await Promise.all([
      fetchInventories(client),
      fetchLinks(client),
      fetchConfigs(client),
    ]);

    return {
      hosts,
      components,
      providers,
      links,
      configs,
    };
  };
}

export {useLatticeData};
