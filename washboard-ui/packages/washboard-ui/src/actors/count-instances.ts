import {WadmActor} from '@/services/lattice';

export function countInstances(instances: WadmActor['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}
