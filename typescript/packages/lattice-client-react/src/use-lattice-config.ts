import * as React from 'react';
import {
  LatticeConfigContext,
  LatticeConfigContextType,
} from './context/lattice-config-context';

/**
 * get the current lattice config and a function to update it
 */
function useLatticeConfig(): LatticeConfigContextType {
  return React.useContext(LatticeConfigContext);
}

export {useLatticeConfig};
