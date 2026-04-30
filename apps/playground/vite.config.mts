/// <reference types='vitest' />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig(() => {
  const backendOrigin =
    process.env.UI_BACKEND_ORIGIN ?? 'http://localhost:8080';
  const proxy = process.env.CI
    ? undefined
    : {
        '/api': backendOrigin,
        '/health': backendOrigin,
      };

  return {
    root: import.meta.dirname,
    cacheDir: '../../node_modules/.vite/apps/playground',
    server: {
      port: 4201,
      host: 'localhost',
      proxy,
    },
    preview: {
      port: 4201,
      host: 'localhost',
    },
    plugins: [react(), tailwindcss()],
    build: {
      outDir: './dist',
      emptyOutDir: true,
      reportCompressedSize: true,
      commonjsOptions: {
        transformMixedEsModules: true,
      },
    },
  };
});
