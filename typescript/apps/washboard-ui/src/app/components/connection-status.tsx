import {canConnect, useLatticeConfig} from '@wasmcloud/lattice-client-react';
import {clsx} from 'clsx';
import * as React from 'react';

export function ConnectionStatus(): React.ReactElement {
  const {config: latticeConfig} = useLatticeConfig();
  const [status, setStatus] = React.useState<'Pending' | 'Online' | 'Offline'>('Pending');
  React.useEffect(() => {
    canConnect(latticeConfig?.latticeUrl).then((online) =>
      setStatus(online ? 'Online' : 'Offline'),
    );
  }, [latticeConfig.latticeUrl]);

  return (
    <div
      data-testid="connection-status"
      data-status={status}
      className={clsx(
        {
          Pending: 'text-gray-500 bg-gray-100/10',
          Online: 'text-green-400 bg-green-400/10',
          Offline: 'text-rose-400 bg-rose-400/10',
        }[status],
        'flex-none rounded-full p-1',
      )}
    >
      <div className="size-2 rounded-full bg-current" />
      <span className="sr-only">Lattice Connection: {status}</span>
    </div>
  );
}
