// Browser tests (Playwright): the live editor mounted in a real page, against the fake server.
// Everything they need is under `client/`. Pick browsers with `--project` (`chromium`, `webkit`);
// a selected browser that can't launch fails the run. See EDITOR.md, "Browser tests".

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "test/browser",
  testMatch: /.*\.spec\.ts$/,
  globalSetup: "./test/browser/global-setup.ts",
  fullyParallel: false,
  workers: 1,
  forbidOnly: true,
  retries: 0,
  timeout: 60_000,
  reporter: [["list"]],
  use: { headless: true, trace: "off" },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
});
