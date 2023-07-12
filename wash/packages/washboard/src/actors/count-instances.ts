import {WadmActor} from 'lattice/lattice-service';

function countInstances(instances: WadmActor['instances']): number {
  return Object.values(instances).reduce((accumulator, current) => accumulator + current.length, 0);
}

export {countInstances};
