// Re-export core
export * from '@wasmcloud/lattice-client-core';

// package exports
export {useLatticeConfig} from './use-lattice-config';
export {useLatticeData} from './use-lattice-data';
export {useLatticeClient, LatticeClientProvider} from './lattice-client-provider';

export type {LatticeClientConfig} from '@wasmcloud/lattice-client-core';
export type {LatticeClientProviderProps} from './lattice-client-provider';
