import {canConnect, useLatticeConfig} from '@wasmcloud/lattice-client-react';
import {clsx} from 'clsx';
import * as React from 'react';

export function ConnectionStatus(): React.ReactElement {
  const [latticeConfig] = useLatticeConfig();
  const [status, setStatus] = React.useState<'PENDING' | 'ONLINE' | 'OFFLINE'>('PENDING');
  React.useEffect(() => {
    canConnect(latticeConfig.latticeUrl).then((online) => setStatus(online ? 'ONLINE' : 'OFFLINE'));
  }, [latticeConfig.latticeUrl]);
  return (
    <div
      className={clsx(
        {
          PENDING: 'text-gray-500 bg-gray-100/10',
          ONLINE: 'text-green-400 bg-green-400/10',
          OFFLINE: 'text-rose-400 bg-rose-400/10',
        }[status],
        'flex-none rounded-full p-1',
      )}
    >
      <div className="size-2 rounded-full bg-current" />
    </div>
  );
}
