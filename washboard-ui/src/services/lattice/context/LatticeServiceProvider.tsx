import * as React from 'react';
import LatticeService from '../classes/lattice-service';

export const LatticeServiceContext = React.createContext<LatticeService | undefined>(undefined);

export type LatticeServiceProviderProps = {
  service: LatticeService;
  children?: React.ReactNode;
};

export const LatticeServiceProvider = ({
  service,
  children,
}: LatticeServiceProviderProps): React.ReactElement => {
  React.useEffect(() => {
    if (!service) return;

    service.connect();

    return () => {
      service.disconnect();
    };
  }, [service]);

  return (
    <LatticeServiceContext.Provider value={service}>{children}</LatticeServiceContext.Provider>
  );
};

export const useLatticeService = (LatticeService?: LatticeService): LatticeService => {
  const client = React.useContext(LatticeServiceContext);

  if (LatticeService) return LatticeService;

  if (!client)
    throw new Error('You must provide a LatticeService instance through <LatticeServiceProvider>');

  return client;
};
