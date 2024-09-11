import path from 'node:path';
import sourceMaps from 'rollup-plugin-sourcemaps';
import {defineConfig} from 'vite';
import dts from 'vite-plugin-dts';
import tsconfigPaths from 'vite-tsconfig-paths';

export default defineConfig({
  plugins: [tsconfigPaths(), dts()],
  build: {
    lib: {
      entry: path.resolve(import.meta.dirname, 'src/index.ts'),
      name: 'lattice-client-core',
      formats: ['es', 'cjs'],
    },
    outDir: 'build',
    rollupOptions: {
      plugins: [sourceMaps()],
    },
    sourcemap: true,
  },
});
