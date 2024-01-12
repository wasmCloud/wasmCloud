import {useDebugValue, useEffect, useState} from 'react';
import {LatticeCache} from '../classes/lattice-service';
import {useLatticeService} from '../context/LatticeServiceProvider';

function useLatticeData(): LatticeCache {
  const service = useLatticeService();
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
