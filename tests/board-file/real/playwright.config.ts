// k2:board-file@1 on a real .k5board against a sandbox k2-daemon. Needs K2_BOARD_DAEMON_BIN (fails without it).
//   K2_BOARD_DAEMON_BIN=target/debug/k2-daemon npx playwright test --config tests/board-file/real/playwright.config.ts
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
    { name: 'webkit', use: { browserName: 'webkit' } },
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
})
