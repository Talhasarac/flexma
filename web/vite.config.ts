import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The Rust server (default :8080) serves /api, /mcp and the built app.
// In development Vite proxies API + WebSocket traffic to it.
const server = process.env.FLEXMA_SERVER ?? 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // The server refuses sockets whose Origin isn't its own host.
      '/api': { target: server, ws: true, changeOrigin: true, rewriteWsOrigin: true },
      '/mcp': { target: server, changeOrigin: true },
      '^/agents/(install\.sh|skills/.*)': { target: server, changeOrigin: true },
      '/.well-known': { target: server, changeOrigin: true },
      '/oauth': { target: server, changeOrigin: true },
    },
  },
  build: {
    target: 'es2022',
    sourcemap: false,
    chunkSizeWarningLimit: 2000,
  },
});
