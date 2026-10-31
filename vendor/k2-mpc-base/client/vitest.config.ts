// `npm test`: the unit and convergence suites. The browser tests (test/browser) are
// Playwright's (`npm run test:browser`).
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  test: { exclude: [...configDefaults.exclude, "test/browser/**"] },
});
