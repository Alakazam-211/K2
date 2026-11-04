// k2:board-file@1 in Chromium and WebKit (a K2 tab's frame is a WKWebView on macOS).
// Run:      npx playwright test --config tests/board-file/playwright.config.ts
import { defineConfig } from '@playwright/test'
import * as path from 'path'

export default defineConfig({
  testDir: __dirname,
  testMatch: /\.spec\.ts$/,
  // real/ needs a built daemon: its own config.
  testIgnore: /\/real\//,
  outputDir: path.join(__dirname, 'test-results'),
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 120_000,
  reporter: [['list']],
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
})
