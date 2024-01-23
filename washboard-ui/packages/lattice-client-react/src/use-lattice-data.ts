import {useDebugValue, useEffect, useState} from 'react';
import {useLatticeClient} from './lattice-client-provider';
import {LatticeCache} from '@wasmcloud/lattice-client-core';

function useLatticeData(): LatticeCache {
  const service = useLatticeClient();
  const [state, handleStateUpdate] = useState<LatticeCache>({
    hosts: {},
    actors: {},
    providers: {},
    links: [],
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
