import {
  LatticeClientOptions,
  LatticeClient,
  LatticeClientProvider,
  NatsWsLatticeConnection,
  LatticeConfigProvider,
} from '@wasmcloud/lattice-client-react';
import * as React from 'react';

type Config = Partial<LatticeClientOptions['config']>;
const getClient = (parameters?: Config) => {
  const config: LatticeClientOptions = {
    config: {
      latticeUrl: import.meta.env.VITE_NATS_WEBSOCKET_URL ?? 'ws://localhost:4223',
      ...parameters,
    },
    getNewConnection: ({latticeUrl}) => new NatsWsLatticeConnection({latticeUrl}),
  };
  return new LatticeClient(config);
};

export function AppLatticeClientProvider({children}: React.PropsWithChildren): React.ReactElement {
  const [client, setClient] = React.useState(getClient());

  const resetClient = (config: Config) => {
    const newClient = getClient(config);
    return setClient(newClient);
  };

  return (
    <LatticeClientProvider client={client}>
      <LatticeConfigProvider setClient={resetClient}>{children}</LatticeConfigProvider>
    </LatticeClientProvider>
  );
}
