import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { resolve } from 'path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared')
    }
  },
  root: 'src/renderer',
  build: {
    outDir: '../../out/renderer',
    emptyOutDir: true
  },
  server: {
    port: 5173,
    strictPort: false
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_']
})
