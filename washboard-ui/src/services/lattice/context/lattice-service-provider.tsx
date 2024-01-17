import * as React from 'react';
import {LatticeService} from '../classes/lattice-service';

export const LatticeServiceContext = React.createContext<LatticeService | undefined>(undefined);

export type LatticeServiceProviderProps = {
  service: LatticeService;
  children?: React.ReactNode;
};

export function LatticeServiceProvider(props: LatticeServiceProviderProps): React.ReactElement {
  React.useEffect(() => {
    if (!props.service) return;

    props.service.connect();

    return () => {
      props.service.disconnect();
    };
  }, [props.service]);

  return (
    <LatticeServiceContext.Provider value={props.service}>
      {props.children}
    </LatticeServiceContext.Provider>
  );
}
