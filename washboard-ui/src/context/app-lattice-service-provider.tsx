import * as React from 'react';
import {LatticeService, LatticeServiceProvider} from '@/services/lattice';

const service = new LatticeService();

export function AppLatticeServiceProvider({children}: React.PropsWithChildren): React.ReactElement {
  return <LatticeServiceProvider service={service}>{children}</LatticeServiceProvider>;
}
