import { defineConfig } from 'vitest/config'
import { resolve } from 'path'

// Home M2 (prd-home-multi-server-client-v1 MS51, vs-live MS74): the client
// half of the two-daemon harness. `bun run test:multiserver` builds nothing:
// it spawns this checkout's own `target/debug/k2-daemon` twice (separate
// temp HOMEs) from `globalSetup` and runs `src/**/*.mstest.ts` against both.
// A missing binary FAILS the run (build it with `cargo build -p k2-daemon`);
// it never skips. Not part of the default `vitest run`.
export default defineConfig({
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer'),
      '@shared': resolve(__dirname, 'src/shared'),
    },
  },
  test: {
    include: ['src/**/*.mstest.ts'],
    environment: 'node',
    globalSetup: ['src/renderer/test-utils/two-daemons.global-setup.ts'],
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
})
