import * as React from 'react';
import LatticeService from './lattice-service';
import {useEffect, useState} from "react";

type SetConfigFunction = <K extends keyof LatticeService>(
  key: K,
  value: K extends keyof LatticeService ? LatticeService[typeof key] : never,
) => void;

interface UseLatticeConfigResult {
  config: {
    latticeUrl: string;
  };
  setConfig: SetConfigFunction;
}

function useLatticeConfig(): UseLatticeConfigResult {
  const service = React.useMemo(() => LatticeService.getInstance(), []);
  const setConfig = React.useCallback<SetConfigFunction>(
    (key, value) => {
      service[key] = value;
    },
    [service],
  );
  return {
    config: {
      latticeUrl: service.latticeUrl,
    },
    setConfig,
  };
}

export function useReactiveConfig() {
  const [config, setConfig] = useState(() => LatticeService.getInstance().config$.value);

  useEffect(() => {
    const subscription = LatticeService.getInstance().config$.subscribe(
      newConfig => setConfig(newConfig)
    );

    return () => subscription.unsubscribe();
  }, []);

  return config;
}

export {useLatticeConfig};
