import * as React from 'react';
import {LatticeClient} from '@wasmcloud/lattice-client-core';

export const LatticeClientContext = React.createContext<LatticeClient | undefined>(undefined);

export type LatticeClientProviderProps = {
  client: LatticeClient;
  children?: React.ReactNode;
};

export function useLatticeClient(LatticeClient?: LatticeClient): LatticeClient {
  const client = React.useContext(LatticeClientContext);

  if (LatticeClient) return LatticeClient;

  if (!client)
    throw new Error('You must provide a LatticeClient instance through <LatticeClientProvider>');

  return client;
}

export function LatticeClientProvider(props: LatticeClientProviderProps): React.ReactElement {
  React.useEffect(() => {
    if (!props.client) return;

    props.client.connect();

    return () => {
      props.client.disconnect();
    };
  }, [props.client]);

  return (
    <LatticeClientContext.Provider value={props.client}>
      {props.children}
    </LatticeClientContext.Provider>
  );
}
