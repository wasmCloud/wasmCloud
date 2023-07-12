import {useDebugValue, useEffect, useMemo, useState} from 'react';
import LatticeService, {LatticeCache} from './lattice-service';

function useLatticeData(): LatticeCache {
  const service = useMemo(() => LatticeService.getInstance(), []);
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

export default useLatticeData;
