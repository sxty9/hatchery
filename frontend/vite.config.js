import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Built assets land in frontend/dist and are served by axum (ServeDir). In dev,
// `vite` proxies the API + WebSocket to the running hatchery-server on :8799.
export default defineConfig({
  plugins: [react()],
  base: './',
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8799',
      '/ws': { target: 'ws://127.0.0.1:8799', ws: true },
    },
  },
})
