import {WasmCloudComponent} from '@wasmcloud/lattice-client-react';

export function countInstances(instances: WasmCloudComponent['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}
