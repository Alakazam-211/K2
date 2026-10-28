// @vitest-environment jsdom
// The app's own wiring of the one-time carry-over: the first read of the
// daemon's "Always" list in an app load first allows every Garden/widget an
// older app kept in localStorage (marked `migrated`), then drops the key;
// later reads carry nothing.
import { afterEach, describe, expect, it, vi } from 'vitest'

const calls: Array<{ method: 'GET' | 'POST'; route: string; body: unknown }> = []
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params: unknown) => {
    calls.push({ method: 'GET', route, body: params })
    return { ok: true, garden: 'g-1', widgets: ['board'] }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body: unknown) => {
    calls.push({ method: 'POST', route, body })
    return { ok: true, added: true }
  }),
}))
vi.mock('./zen-api', () => ({ zenLocalScope: () => ({ id: 'local' }) }))

import { __resetZenDownloadCarryForTests, daemonZenDownloadConsent, LEGACY_DOWNLOAD_CONSENT_KEY } from './zen-download-consent'

afterEach(() => {
  calls.length = 0
  localStorage.clear()
  __resetZenDownloadCarryForTests()
})

describe('the app carries an older stored Always before its first read', () => {
  it('posts each pair as migrated, drops the key, then reads; once per load', async () => {
    localStorage.setItem(LEGACY_DOWNLOAD_CONSENT_KEY, JSON.stringify(['g-1/board', 'g-2/notes']))
    expect(await daemonZenDownloadConsent.widgets('g-1')).toEqual(['board'])
    expect(calls).toEqual([
      { method: 'POST', route: 'zen/downloads/allow', body: { garden: 'g-1', widget: 'board', migrated: true } },
      { method: 'POST', route: 'zen/downloads/allow', body: { garden: 'g-2', widget: 'notes', migrated: true } },
      { method: 'GET', route: 'zen/downloads', body: { garden: 'g-1' } },
    ])
    expect(localStorage.getItem(LEGACY_DOWNLOAD_CONSENT_KEY)).toBeNull()
    calls.length = 0
    await daemonZenDownloadConsent.widgets('g-1')
    expect(calls.map((c) => c.method)).toEqual(['GET'])
  })

  it('a card answer of Always is a plain allow (not migrated)', async () => {
    await daemonZenDownloadConsent.allow('g-1', 'board')
    expect(calls).toEqual([{ method: 'POST', route: 'zen/downloads/allow', body: { garden: 'g-1', widget: 'board' } }])
  })
})
