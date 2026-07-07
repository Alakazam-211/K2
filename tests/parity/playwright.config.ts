// Playwright config for the Style System screenshot PARITY harness
// (prd-style-system-v1 §6, P1 / Checkpoint A).
//
// Deliberately its OWN config file: the repo's vitest setup
// (vitest.config.ts, src/**/*.test.ts) is untouched — this harness is
// invoked only via `bun run parity:capture`.
//
// Engine: WebKit ONLY. The app ships inside a WKWebView; Chromium/Firefox
// rasterize text, shadows, and subpixel geometry differently, which would
// poison a strict 0-pixel diff against what users actually see.
import { defineConfig } from '@playwright/test'
import * as path from 'path'

// Dedicated port so the harness never collides with (or silently reuses)
// a developer's normal `bun run vite:dev` on 5173. --strictPort makes a
// port conflict a loud failure instead of a silent drift to another port.
export const PARITY_PORT = 5199
const repoRoot = path.resolve(__dirname, '..', '..')

export default defineConfig({
  testDir: __dirname,
  testMatch: /capture\.spec\.ts/,
  outputDir: path.join(__dirname, 'test-results'),
  // Serial + single worker: one vite server, deterministic capture order.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  reporter: [['list']],
  use: {
    browserName: 'webkit',
    // Fixed raster geometry — any change here invalidates cross-run diffs.
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: 'dark',
    reducedMotion: 'reduce',
    baseURL: `http://localhost:${PARITY_PORT}/`,
  },
  webServer: {
    command: `bun run vite:dev --port ${PARITY_PORT} --strictPort`,
    cwd: repoRoot,
    url: `http://localhost:${PARITY_PORT}/`,
    // Reuse a leftover parity server between local runs; CI always cold-starts.
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
})
