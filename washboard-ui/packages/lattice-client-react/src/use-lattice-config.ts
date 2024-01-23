import {LatticeClientConfig} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {useLatticeClient} from './lattice-client-provider';

type SetConfigFunction = (value: Partial<LatticeClientConfig>) => void;

type UseLatticeConfigResult = [LatticeClientConfig, SetConfigFunction];

/**
 * get the current lattice config and a function to update it
 */
function useLatticeConfig(): UseLatticeConfigResult {
  const client = useLatticeClient();
  const [config, setConfigState] = React.useState<LatticeClientConfig>(client.config$.value);
  const setConfig = React.useCallback<SetConfigFunction>(
    (value) => client.setPartialConfig(value),
    [client],
  );

  React.useEffect(() => {
    const subscription = client.config$.subscribe((newConfig) => setConfigState(newConfig));

    return () => subscription.unsubscribe();
  }, [client]);

  return [config, setConfig];
}

export {useLatticeConfig};
