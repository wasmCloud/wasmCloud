import typescript from '@rollup/plugin-typescript';
import { nodeResolve } from '@rollup/plugin-node-resolve';

export default {
  input: 'src/http-password-checker.ts',
  output: {
    file: 'dist/http-password-checker.js',
    format: 'esm',
  },
  plugins: [
    typescript(),
    // NOTE: we use rollup & the nodeResolve plugin here to ensure that all ndoe dependencies
    // are bundled into a *single* file.
    //
    // see: https://github.com/rollup/plugins/tree/master/packages/node-resolve
    nodeResolve(),
  ],
};
