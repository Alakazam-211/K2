import { defineConfig } from 'vitest/config'
import { resolve } from 'path'

// Headless WebKit render checks (0.46.3, Garden gaps): `*.wktest.ts` drive
// Playwright's WebKit (the engine of the desktop app's webview) to render
// real Garden widget frames and read pixels back. `bun run test:webkit`.
// Needs Playwright's WebKit build (`npx playwright install webkit`); a
// missing browser FAILS the run, it never skips. Not part of `vitest run`.
export default defineConfig({
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared'),
    },
  },
  test: {
    include: ['src/**/*.wktest.ts'],
    environment: 'node',
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
})
