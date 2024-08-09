import {type LatticeEvent} from '@wasmcloud/lattice-client-core';
import * as React from 'react';
import {useLatticeClient} from '@/context/use-lattice-client';

function useLatticeEvents(callback: (event: LatticeEvent) => void) {
  const client = useLatticeClient();

  React.useEffect(() => {
    const sub = client.instance.subscribe(`${client.instance.config.ctlTopic}.>`, (event) => {
      callback(event);
    });
    return () => {
      sub.unsubscribe();
    };
  }, [client, callback]);
}

export {useLatticeEvents};
