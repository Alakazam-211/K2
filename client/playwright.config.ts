// Browser tests (Playwright): the live editor mounted in a real page, against the fake server.
// Everything they need is under `client/`. Chromium only by default; `MPC_BROWSERS` names the
// browsers to run instead (`chromium`, `webkit`, comma-separated, e.g. `MPC_BROWSERS=webkit`),
// so a bare `playwright test` never needs WebKit's system libraries. A name it doesn't know
// fails the run, and so does a selected browser that can't launch. See EDITOR.md, "Tests".

import { defineConfig, devices } from "@playwright/test";

const ALL = {
  chromium: { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  webkit: { name: "webkit", use: { ...devices["Desktop Safari"] } },
};

function browsers(): Array<(typeof ALL)[keyof typeof ALL]> {
  const names = (process.env.MPC_BROWSERS ?? "chromium")
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
  if (names.length === 0) throw new Error("MPC_BROWSERS names no browser");
  return names.map((n) => {
    if (!(n in ALL)) throw new Error(`MPC_BROWSERS: unknown browser "${n}" (known: ${Object.keys(ALL).join(", ")})`);
    return ALL[n as keyof typeof ALL];
  });
}

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
  projects: browsers(),
});
