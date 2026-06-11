// Smoke test: proves vitest is wired into the repo. Delete once
// real tests (Phase 4.5 I3+) replace it. Every future TS test
// colocates with its source file as `<name>.test.ts`.
import { describe, it, expect } from 'vitest'

describe('vitest infra', () => {
  it('runs a trivial assertion', () => {
    expect(1 + 1).toBe(2)
  })

  it('resolves @ alias', async () => {
    // If the @ alias is wired, this import should succeed. We use
    // a module that has no side-effects on import.
    const mod = await import('@/lib/key-mapping')
    expect(typeof mod.MODE_APP_CURSOR).toBe('number')
  })
})
