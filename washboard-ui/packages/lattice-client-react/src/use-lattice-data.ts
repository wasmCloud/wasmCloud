import {LatticeCache} from '@wasmcloud/lattice-client-core';
import {useDebugValue, useEffect, useState} from 'react';
import {useLatticeClient} from './lattice-client-provider';

function useLatticeData(): LatticeCache {
  const service = useLatticeClient();
  const [state, handleStateUpdate] = useState<LatticeCache>({
    hosts: {},
    components: {},
    providers: {},
    links: [],
    configs: {},
  });
  useDebugValue(state);

  useEffect(() => {
    const sub = service.getLatticeCache$().subscribe(handleStateUpdate);
    return () => {
      sub.unsubscribe();
    };
  }, [service]);

  return state;
}

export {useLatticeData};
