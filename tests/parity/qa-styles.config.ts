// Playwright config for the Style System QA screenshot matrix
// (qa-styles.spec.ts) — non-default style configurations (Paper /
// Spacious). Reuses the parity harness's engine, geometry and vite
// server verbatim; only the testMatch differs, so the strict parity
// capture (capture.spec.ts, PARITY_LABEL-gated) is never picked up here.
import { defineConfig } from '@playwright/test'
import baseConfig from './playwright.config'

export default defineConfig({
  ...baseConfig,
  testMatch: /qa-styles\.spec\.ts/,
})
