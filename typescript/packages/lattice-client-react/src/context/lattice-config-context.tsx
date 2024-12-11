import {type LatticeClient, type LatticeClientOptions} from '@wasmcloud/lattice-client-core';
import * as React from 'react';

export type LatticeClientConfigOutput = typeof LatticeClient.prototype.instance.config;
export type LatticeClientConfigInput = Partial<LatticeClientOptions['config']>;

export type SetConfigFunction = (value: LatticeClientConfigInput) => void;

export type LatticeConfigContextType = {
  config: LatticeClientConfigOutput;
  setConfig: SetConfigFunction;
};

export const LatticeConfigContext = React.createContext<LatticeConfigContextType>({
  config: {} as LatticeClientConfigOutput,
  setConfig: () => {},
});
 