import * as React from 'react';
import {LatticeService} from '../classes/lattice-service';
import {LatticeServiceContext} from '../context/lattice-service-provider';

export function useLatticeService(LatticeService?: LatticeService): LatticeService {
  const client = React.useContext(LatticeServiceContext);

  if (LatticeService) return LatticeService;

  if (!client)
    throw new Error('You must provide a LatticeService instance through <LatticeServiceProvider>');

  return client;
}
