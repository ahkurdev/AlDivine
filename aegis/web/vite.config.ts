import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      '/aegis': { target: 'http://127.0.0.1:40120', changeOrigin: true, rewrite: (p) => p.replace(/^\/aegis/, '') },
    },
  },
})
