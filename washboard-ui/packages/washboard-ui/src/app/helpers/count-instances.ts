import {WadmComponent} from '@wasmcloud/lattice-client-react';

export function countInstances(instances: WadmComponent['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}
