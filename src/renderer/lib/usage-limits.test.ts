import { describe, expect, it } from 'vitest'
import { findLimitsLogin, limitsStrip, parseLimitsDoc } from './usage-limits'
import { createKeyedPollers } from './keyed-poller'

const NOW = Date.parse('2026-10-16T12:00:00Z')
const iso = (ms: number): string => new Date(ms).toISOString()
const H = 3_600_000

function login(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    accountId: 'acc_work',
    label: 'work',
    email: 'w@example.test',
    kind: 'subscription',
    isDefault: true,
    isLive: true,
    pinned: [],
    state: 'ok',
    status: '',
    error: null,
    plan: 'Max 20x',
    checkedAt: iso(NOW - 60_000),
    attemptedAt: null,
    source: 'probe',
    nextProbeAt: null,
    windows: [
      { id: 'five_hour', label: 'Session', kind: 'session', used: 0.2, usedPercent: 20, resetsAt: iso(NOW + 3 * H), windowSecs: 18000 },
      { id: 'seven_day', label: 'Weekly', kind: 'weekly', used: 0.62, usedPercent: 62, resetsAt: iso(NOW + 30 * H), windowSecs: 604800 },
      { id: 'model:fable:weekly', label: 'Fable Weekly', kind: 'weekly', used: 0.99, usedPercent: 99, resetsAt: iso(NOW + 30 * H) },
    ],
    ...extra,
  }
}

const DOC = {
  generatedAt: iso(NOW),
  airgap: false,
  tools: [
    {
      tool: 'claude',
      display: 'Claude',
      supported: true,
      logins: [
        login(),
        login({ accountId: 'acc_home', label: 'home', isDefault: false, isLive: false, pinned: [{ scopeKind: 'workspace', scopeId: 'p1', label: 'K2' }] }),
      ],
    },
    {
      tool: 'codex',
      display: 'Codex',
      supported: true,
      logins: [
        login({
          accountId: null,
          label: 'Codex',
          kind: 'unmanaged',
          windows: [{ id: 'primary', label: '5h', used: 0.18, resetsAt: iso(NOW - 1000), resetPassed: true }],
        }),
      ],
    },
    { tool: 'grok', display: 'Grok', supported: true, logins: [login({ accountId: 'acc_g', state: 'expired', status: 'Sign-in expired', windows: [] })] },
    { tool: 'gemini', display: 'Gemini', supported: false, logins: [login({ accountId: 'acc_key', kind: 'api_key', state: 'unsupported', windows: [] })] },
  ],
}

describe('parseLimitsDoc', () => {
  it('keeps every login, its pins, the typed state and the window fields', () => {
    const doc = parseLimitsDoc(DOC)
    expect(doc.tools.map((t) => t.tool)).toEqual(['claude', 'codex', 'grok', 'gemini'])
    const home = findLimitsLogin(doc, 'acc_home')
    expect(home?.pinned).toEqual([{ scopeKind: 'workspace', scopeId: 'p1', label: 'K2' }])
    expect(home?.isDefault).toBe(false)
    const work = findLimitsLogin(doc, 'acc_work')
    expect(work?.windows.map((w) => w.label)).toEqual(['Session', 'Weekly', 'Fable Weekly'])
    expect(work?.windows[1].windowSecs).toBe(604800)
    expect(work?.state).toBe('ok')
    expect(work?.source).toBe('probe')
    expect(work?.harness).toBe('claude')
    expect(doc.tools[1].logins[0].accountId).toBeNull()
    expect(findLimitsLogin(doc, 'nope')).toBeNull()
  })

  it('turns anything else into an empty doc', () => {
    for (const raw of [null, 'x', [], { tools: {} }, { error: 'admin_only' }]) {
      expect(parseLimitsDoc(raw)).toEqual({ generatedAt: '', airgap: false, tools: [] })
    }
    expect(parseLimitsDoc({ tools: [{ display: 'no id' }, { tool: 'claude', logins: [null, 3] }] }).tools).toEqual([
      { tool: 'claude', display: 'Claude', supported: false, logins: [] },
    ])
  })
})

describe('limitsStrip', () => {
  it('one item per supported tool: the default login, highest main window, reset countdown', () => {
    const strip = limitsStrip(parseLimitsDoc(DOC), NOW)
    // Claude: Weekly 62% (the model-scoped Fable week stays on Subs / Tokens).
    expect(strip[0]).toEqual({ tool: 'claude', display: 'Claude', percent: 62, note: 'resets 30h', warn: false })
    // Codex: the window passed its reset: 0 until the re-check lands.
    expect(strip[1]).toEqual({ tool: 'codex', display: 'Codex', percent: 0, note: 'reset — re-checking', warn: false })
    // Grok: expired sign-in says so and warns; Gemini (unsupported) is left out.
    expect(strip[2]).toEqual({ tool: 'grok', display: 'Grok', percent: null, note: 'sign-in expired', warn: true })
    expect(strip).toHaveLength(3)
  })

  it('is empty in air-gap mode or without a doc', () => {
    expect(limitsStrip(parseLimitsDoc({ ...DOC, airgap: true }), NOW)).toEqual([])
    expect(limitsStrip(null, NOW)).toEqual([])
  })
})

describe('createKeyedPollers (B32)', () => {
  it('starts one poller per key and stops it at the last release', () => {
    const pollers = createKeyedPollers()
    let starts = 0
    let stops = 0
    const start = (): (() => void) => {
      starts += 1
      return () => {
        stops += 1
      }
    }
    const a = pollers.acquire('srv', start)
    const b = pollers.acquire('srv', start)
    const c = pollers.acquire('other', start)
    expect(starts).toBe(2)
    expect(pollers.holds('srv')).toBe(2)
    a()
    a() // a second release of the same hold is a no-op
    expect(stops).toBe(0)
    expect(pollers.holds('srv')).toBe(1)
    b()
    expect(stops).toBe(1)
    expect(pollers.holds('srv')).toBe(0)
    pollers.stopAll()
    expect(stops).toBe(2)
    c() // released after a reset: nothing left to stop
    expect(stops).toBe(2)
  })
})
