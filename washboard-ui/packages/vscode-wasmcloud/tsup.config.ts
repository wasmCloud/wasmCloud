import {defineConfig} from 'tsup';

export default defineConfig({
  entry: ['src/extension.ts'],
  format: ['cjs'],
  shims: false,
  dts: false,
  external: ['vscode'],
});
