// The built-in Board widget (k2:board@1) in real browsers: Chromium and WebKit (the Garden is a WKWebView on
// macOS, and WebKit is where a canvas comes back empty in K2's sealed frame).
//
// The widget's folder is served as a static site by Playwright's router (no server, no network), with the
// stand-in `k2` in tests/board/fake-k2.js. Its JSDOM suite is src/renderer/lib/contract/builtin-board.test.ts.
//
// Run:      bun run board:test   (or npx playwright test --config tests/board/playwright.config.ts)
// Browsers: npx playwright install chromium webkit
import { defineConfig } from '@playwright/test'
import * as path from 'path'

export default defineConfig({
  testDir: __dirname,
  testMatch: /\.spec\.ts$/,
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
