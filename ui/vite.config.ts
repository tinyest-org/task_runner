import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import tailwindcss from '@tailwindcss/vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

export default defineConfig({
  plugins: [solid(), tailwindcss(), viteSingleFile()],
  root: '.',
  build: {
    outDir: '../static',
    emptyOutDir: false,
  },
  server: {
    proxy: {
      '/dag': 'http://localhost:8085',
      '/task': 'http://localhost:8085',
      '/batch': 'http://localhost:8085',
      '/batches': 'http://localhost:8085',
    },
  },
});
