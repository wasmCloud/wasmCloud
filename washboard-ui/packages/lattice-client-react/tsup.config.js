// @ts-check
import {defineConfig} from 'tsup';

export default defineConfig({
  entry: ['src/**/*.ts', 'src/**/*.tsx'],
  format: ['cjs', 'esm'],
  target: ['chrome91', 'firefox90', 'edge91', 'safari15', 'ios15', 'opera77'],
  outDir: 'build',
  dts: true,
  sourcemap: true,
  clean: true,
});
