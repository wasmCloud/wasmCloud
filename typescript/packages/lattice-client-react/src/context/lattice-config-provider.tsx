import type {
  LatticeClientConfigOutput,
  LatticeClientConfigInput,
  SetConfigFunction,
} from './lattice-config-context';
import * as React from 'react';
import {useLatticeClient} from '@/context/use-lattice-client';
import {LatticeConfigContext} from './lattice-config-context';

export function LatticeConfigProvider({
  children,
  setClient,
}: React.PropsWithChildren & {
  setClient: (config: LatticeClientConfigInput) => void;
}): React.ReactElement {
  const client = useLatticeClient();
  const [config, setConfigState] = React.useState<LatticeClientConfigOutput>(
    client.instance.config,
  );

  const setConfig = React.useCallback<SetConfigFunction>(
    (newConfig) => {
      client.instance.disconnect();
      setClient(newConfig);
    },
    [client, setClient],
  );

  React.useEffect(() => {
    setConfigState(() => client.instance.config);
  }, [client]);
  return (
    <LatticeConfigContext.Provider
      value={{
        config,
        setConfig,
      }}
    >
      {children}
    </LatticeConfigContext.Provider>
  );
}
