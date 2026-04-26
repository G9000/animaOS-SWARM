/// <reference types='vitest' />
import { defineConfig, type ProxyOptions } from 'vite';
import react from '@vitejs/plugin-react';

type ViteProxy = Parameters<NonNullable<ProxyOptions['configure']>>[0];

function isHarmlessWebSocketShutdown(error: unknown): boolean {
  return (
    error instanceof Error &&
    'code' in error &&
    (error.code === 'ECONNRESET' || error.code === 'ECONNABORTED')
  );
}

function configureWebSocketProxy(proxy: ViteProxy) {
  if (process.env.UI_SUPPRESS_WS_PROXY_RESET !== '1') {
    return;
  }

  queueMicrotask(() => {
    for (const listener of proxy.listeners('error')) {
      proxy.off('error', listener);
    }

    for (const listener of proxy.listeners('proxyReqWs')) {
      proxy.off('proxyReqWs', listener);
    }

    proxy.on('error', (error, _req, socket) => {
      if (isHarmlessWebSocketShutdown(error)) {
        if (typeof socket.end === 'function') {
          socket.end();
        }
        return;
      }

      console.error('[vite] ws proxy error:', error);
      if (typeof socket.end === 'function') {
        socket.end();
      }
    });

    proxy.on('proxyReqWs', (_proxyReq, _req, socket) => {
      socket.on('error', (error) => {
        if (isHarmlessWebSocketShutdown(error)) {
          return;
        }

        console.error('[vite] ws proxy socket error:', error);
      });
    });
  });
}

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
    cacheDir: '../../node_modules/.vite/apps/ui',
    server: {
      port: 4200,
      host: 'localhost',
      proxy,
    },
    preview: {
      port: 4200,
      host: 'localhost',
    },
    plugins: [react()],
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
