/**
 * Hosted web client Vite build variant (Stage A).
 *
 * Produces a static SPA under out/web/app/<version>/ with Tauri API packages
 * aliased to browser shims. Does not alter vite.config.ts (desktop/Tauri).
 */
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { resolve, dirname } from 'path'
import { readFileSync } from 'fs'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))

const pkg = JSON.parse(
  readFileSync(resolve(__dirname, 'package.json'), 'utf-8'),
) as { version: string }

const version: string = pkg.version || 'dev'
const shim = (name: string) => resolve(__dirname, `src/renderer/shims/${name}`)

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared'),
      // Tauri API → browser shims (exact import paths used by the renderer)
      '@tauri-apps/api/core': shim('tauri-core.ts'),
      '@tauri-apps/api/event': shim('tauri-event.ts'),
      '@tauri-apps/api/window': shim('tauri-window.ts'),
      '@tauri-apps/api/webview': shim('tauri-webview.ts'),
      '@tauri-apps/api/app': shim('tauri-app.ts'),
      '@tauri-apps/plugin-opener': shim('plugin-opener.ts'),
      '@tauri-apps/plugin-clipboard-manager': shim('plugin-clipboard-manager.ts'),
      '@tauri-apps/plugin-notification': shim('plugin-notification.ts'),
      '@tauri-apps/plugin-updater': shim('plugin-updater.ts'),
      '@crabnebula/tauri-plugin-drag': shim('plugin-drag.ts'),
    },
  },
  root: 'src/renderer',
  base: `/app/${version}/`,
  define: {
    'import.meta.env.VITE_WEB': JSON.stringify(true),
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(version),
  },
  build: {
    outDir: `../../out/web/app/${version}`,
    emptyOutDir: true,
  },
  server: {
    port: 5174,
    strictPort: false,
    // Same-origin data plane during `vite:dev:web` — point at a local
    // daemon (K2_DAEMON_PORT or default 0 so proxy is inert until set).
    proxy: process.env.K2_DAEMON_PORT
      ? {
          '/boot-status': {
            target: `http://127.0.0.1:${process.env.K2_DAEMON_PORT}`,
            changeOrigin: true,
          },
          '/cli': {
            target: `http://127.0.0.1:${process.env.K2_DAEMON_PORT}`,
            changeOrigin: true,
            ws: true,
          },
          '/events': {
            target: `http://127.0.0.1:${process.env.K2_DAEMON_PORT}`,
            changeOrigin: true,
            ws: true,
          },
        }
      : undefined,
  },
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_'],
})
