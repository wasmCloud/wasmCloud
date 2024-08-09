import {type LatticeClient} from '@wasmcloud/lattice-client-core';
import * as React from 'react';

export const LatticeClientContext = React.createContext<LatticeClient | undefined>(undefined);
