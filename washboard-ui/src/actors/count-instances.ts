import {WadmActor} from '@/services/lattice';

function countInstances(instances: WadmActor['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}

export {countInstances};
