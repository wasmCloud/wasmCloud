import path from 'node:path';
import react from '@vitejs/plugin-react';
import sourceMaps from 'rollup-plugin-sourcemaps';
import {defineConfig} from 'vite';
import dts from 'vite-plugin-dts';
import tsconfigPaths from 'vite-tsconfig-paths';

export default defineConfig({
  plugins: [react(), tsconfigPaths(), dts()],
  resolve: {alias: {'@/': path.resolve('src/')}},
  build: {
    lib: {
      entry: path.resolve(import.meta.dirname, 'src/index.ts'),
      name: 'lattice-client-react',
      formats: ['es' as const, 'cjs' as const],
    },
    outDir: 'build',
    sourcemap: true,
    rollupOptions: {
      plugins: [sourceMaps()],
      external: ['react', 'react-dom', 'react/jsx-runtime'],
      output: {
        globals: {
          react: 'React',
          'react-dom': 'ReactDOM',
          'react/jsx-runtime': 'jsxRuntime',
        },
      },
    },
  },
});
