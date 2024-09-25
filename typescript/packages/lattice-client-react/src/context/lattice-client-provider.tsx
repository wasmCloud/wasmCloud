import {type LatticeClient} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {LatticeClientContext} from '@/context/lattice-client-context';

export type LatticeClientProviderProps = {
  readonly client: LatticeClient;
  readonly children?: React.ReactNode;
};

export function LatticeClientProvider({
  client,
  children,
}: LatticeClientProviderProps): React.ReactElement {
  React.useEffect(() => {
    if (!client) return;

    // if the connection is already in the process of being established, don't do anything
    if (client.connection.status !== 'initial') return;

    client.instance.connect();

    return () => {
      client.instance.disconnect();
    };
  }, [client]);

  return <LatticeClientContext.Provider value={client}>{children}</LatticeClientContext.Provider>;
}
