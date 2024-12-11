import react from '@vitejs/plugin-react';
import sourceMaps from 'rollup-plugin-sourcemaps';
import {defineConfig} from 'vite';
import svgrPlugin from 'vite-plugin-svgr';
import tsconfigPaths from 'vite-tsconfig-paths';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tsconfigPaths(), svgrPlugin()],
  build: {
    sourcemap: true,
    rollupOptions: {
      plugins: [sourceMaps()],
    },
  },
  server: {
    sourcemapIgnoreList: false,
  },
});
