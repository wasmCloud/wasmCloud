import * as React from 'react';
import {LatticeServiceConfig} from '../classes/lattice-service';
import {useLatticeService} from './use-lattice-service';

type SetConfigFunction = (value: Partial<LatticeServiceConfig>) => void;

type UseLatticeConfigResult = [LatticeServiceConfig, SetConfigFunction];

function useLatticeConfig(): UseLatticeConfigResult {
  const service = useLatticeService();
  const [config, setConfigState] = React.useState<LatticeServiceConfig>(service.config$.value);
  const setConfig = React.useCallback<SetConfigFunction>(
    (value) => {
      service.setConfig(value);
    },
    [service],
  );

  React.useEffect(() => {
    const subscription = service.config$.subscribe((newConfig) => setConfigState(newConfig));

    return () => subscription.unsubscribe();
  }, [service]);

  return [config, setConfig];
}

export {useLatticeConfig};
