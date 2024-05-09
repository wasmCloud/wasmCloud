import {type LatticeClient} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {LatticeClientContext} from '@/context/lattice-client-context';

export function useLatticeClient(LatticeClient?: LatticeClient): LatticeClient {
  const client = React.useContext(LatticeClientContext);

  if (LatticeClient) return LatticeClient;

  if (!client)
    throw new Error('You must provide a LatticeClient instance through <LatticeClientProvider>');

  return client;
}
