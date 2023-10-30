import {useEffect, useState} from "react";
import {clsx} from "clsx";
import {useReactiveConfig} from "@/lattice/use-lattice-config.ts";
import {canConnect} from "@/services/nats.ts";

export function ConnectionStatus() {
  const latticeConfig = useReactiveConfig();
  const [status, setStatus] = useState<'PENDING' | 'ONLINE' | 'OFFLINE'>('PENDING');
  useEffect(() => {
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
        'flex-none rounded-full p-1'
      )}
    >
      <div className="h-2 w-2 rounded-full bg-current"/>
    </div>
  )
}
