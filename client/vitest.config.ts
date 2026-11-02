// `npm test`: the unit, convergence and host-contract suites, all in Node (the library has no
// DOM; a host's editor tests run in its own repo).
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {},
});
