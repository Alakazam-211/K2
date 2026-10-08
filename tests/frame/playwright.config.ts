// Real-browser frame-ancestors checks (prd-app-frame-ancestors-v1 §5 test 7,
// FA15, FA19). Its own config: not the tests/parity screenshot harness.
//
// Starts `k2-daemon --skin-gateway` helpers from THIS checkout's build
// (never a running daemon, never ~/.k2) plus a tiny framing page on another
// origin, in Chromium and WebKit:
//   - default / 'none' policy: a foreign page cannot render the App;
//   - allow-listed origin: the App really renders in the frame, with
//     X-Frame-Options: SAMEORIGIN still sent (engines ignore XFO when the
//     CSP has frame-ancestors — FA19's condition for keeping XFO).
//
// Build first:  cargo build -p k2-daemon --bin k2-daemon
// Run:          bun run frame:test   (or npx playwright test --config tests/frame/playwright.config.ts)
// Binary:       $K2_FRAME_TEST_DAEMON_BIN, else $CARGO_TARGET_DIR/debug/k2-daemon,
//               else <repo>/target/debug/k2-daemon. Missing binary fails the run.
// Browsers:     npx playwright install chromium webkit
//
// Not covered here (needs a signed build; see the PRD S0 / FA9): WKWebView
// and WebKitGTK matching `tauri://localhost`, WebView2 matching
// `http://tauri.localhost`.
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
  timeout: 60_000,
  reporter: [['list']],
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
})
