import {type LatticeClient, type LatticeClientOptions} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {useLatticeClient} from './context/use-lattice-client';

type LatticeClientConfigOutput = typeof LatticeClient.prototype.instance.config;
type LatticeClientConfigInput = Partial<LatticeClientOptions['config']>;

type SetConfigFunction = (value: LatticeClientConfigInput) => void;

type UseLatticeConfigResult = [LatticeClientConfigOutput, SetConfigFunction];

/**
 * get the current lattice config and a function to update it
 */
function useLatticeConfig(): UseLatticeConfigResult {
  const client = useLatticeClient();
  const [config, setConfigState] = React.useState<LatticeClientConfigOutput>(
    client.instance.config,
  );
  const setConfig = React.useCallback<SetConfigFunction>(
    (newConfig) => {
      client.instance.setPartialConfig(newConfig);
      setConfigState(() => client.instance.config);
    },
    [client],
  );

  return [config, setConfig];
}

export {useLatticeConfig};
