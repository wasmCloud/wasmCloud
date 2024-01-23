import {WadmActor} from '@wasmcloud/lattice-client-react';

export function countInstances(instances: WadmActor['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}
