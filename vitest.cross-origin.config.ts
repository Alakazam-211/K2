import { defineConfig, mergeConfig } from 'vitest/config'
import base from './vitest.config'

// The cross-origin sweep (src/renderer/test-utils/cross-origin-sweep.setup.ts):
// the whole suite with a sandboxed frame on the page and React's dev render
// logging on, to find props that reach a frame's window. Full runs go to
// K2 Compute, like any full vitest run.
export default mergeConfig(base, defineConfig({ test: { setupFiles: ['src/renderer/test-utils/cross-origin-sweep.setup.ts'] } }))
