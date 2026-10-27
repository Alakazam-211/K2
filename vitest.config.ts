import { defineConfig } from 'vitest/config'
import { resolve } from 'path'

// Vitest runs alongside the Vite dev server but has its own config
// file so test runs don't inherit Tauri-specific build settings
// (asset paths, envPrefix gates, etc.). Re-uses the `@` / `@shared`
// path aliases so tests can import modules the same way production
// code does.
//
// Convention: colocate tests with source using a `.test.ts` suffix —
// `terminalGrid.ts` gets `terminalGrid.test.ts` in the same folder.
// The default `include` glob below picks them up everywhere under
// `src/`, no per-folder wiring required.
export default defineConfig({
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared'),
      // 0.46.5 activity SDK: @k2/activity's core (one copy of the turn words).
      '@k2/activity': resolve(__dirname, 'sdk/activity/src/core.ts'),
    },
  },
  test: {
    // sdk/server: the App server SDK's unit tests (runtime S23).
    // sdk/activity: @k2/activity, the activity SDK's core (0.46.5).
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx', 'sdk/server/test/**/*.test.ts', 'sdk/activity/test/**/*.test.ts'],
    environment: 'node',
  },
})
