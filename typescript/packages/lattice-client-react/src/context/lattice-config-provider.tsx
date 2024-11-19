import {type LatticeClient, type LatticeClientOptions} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {useLatticeClient} from '@/context/use-lattice-client';

type LatticeClientConfigOutput = typeof LatticeClient.prototype.instance.config;
type LatticeClientConfigInput = Partial<LatticeClientOptions['config']>;

type SetConfigFunction = (value: LatticeClientConfigInput) => void;

export type LatticeConfigContextType = {
  config: LatticeClientConfigOutput;
  setConfig: SetConfigFunction;
};

export const LatticeConfigContext = React.createContext<LatticeConfigContextType>({
  config: {} as LatticeClientConfigOutput,
  setConfig: () => {},
});

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
