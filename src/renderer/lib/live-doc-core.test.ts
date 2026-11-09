// @vitest-environment jsdom

// Whether a file opens in the Live editor (0.46.7): the daemon's
// `GET /cli/docs/cores?path=` `file: {core, on, liveView}`, re-asked on
// `doc_cores_changed`; anything else (an older server, an error, a core with
// no live editor) keeps today's view.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  file: null as unknown,
  gets: [] as Array<{ route: string; params: unknown }>,
  fail: false,
}))
const events = vi.hoisted(() => ({ handlers: new Set<(e: unknown) => void>() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: async (_scope: unknown, route: string, params: unknown) => {
    h.gets.push({ route, params })
    if (h.fail) throw new Error('offline')
    return { ok: true, cores: [], file: h.file }
  },
}))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocCoresChanged: (_scope: unknown, fn: (e: unknown) => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))

import { parseLiveDocCore, useLiveDocCore } from './live-doc-core'
import type { ServerScope } from '@/kessel/server-scope'

function scopeWith(features: string[]): ServerScope {
  return { connectionKey: 'k', serverSupports: (f: string) => features.includes(f) } as unknown as ServerScope
}

afterEach(() => {
  h.file = null
  h.gets = []
  h.fail = false
  events.handlers.clear()
})

describe('parseLiveDocCore', () => {
  it('reads file {core, on, liveView} and refuses anything else', () => {
    expect(parseLiveDocCore({ file: { core: 'markdown', on: true, liveView: true } })).toEqual({ core: 'markdown', on: true, liveView: true })
    expect(() => parseLiveDocCore({ cores: [] })).toThrow(/no `file`/)
    expect(() => parseLiveDocCore({ file: { core: 'markdown', on: 'yes', liveView: true } })).toThrow(/core, on, liveView/)
  })
})

describe('useLiveDocCore', () => {
  it('is live while the file’s core is on, and follows doc_cores_changed', async () => {
    h.file = { core: 'markdown', on: true, liveView: true }
    const scope = scopeWith(['docs-cores-v1'])
    const { result } = renderHook(() => useLiveDocCore(scope, '/w/notes.md'))
    await waitFor(() => expect(result.current).toEqual({ status: 'live', core: 'markdown' }))
    expect(h.gets[0]).toEqual({ route: 'docs/cores', params: { path: '/w/notes.md' } })
    h.file = { core: 'markdown', on: false, liveView: true }
    await act(async () => {
      for (const fn of events.handlers) fn({ kind: 'doc_cores_changed', core: 'markdown', on: false })
    })
    await waitFor(() => expect(result.current).toEqual({ status: 'off' }))
  })

  it('gives the any-file panel while its core is on, and follows the switch off', async () => {
    h.file = { core: 'any-file', on: true, liveView: false }
    const scope = scopeWith(['docs-cores-v1'])
    const { result } = renderHook(() => useLiveDocCore(scope, '/w/poster.psd'))
    await waitFor(() => expect(result.current).toEqual({ status: 'any-file' }))
    h.file = { core: 'any-file', on: false, liveView: false }
    await act(async () => {
      for (const fn of events.handlers) fn({ kind: 'doc_cores_changed', core: 'any-file', on: false })
    })
    await waitFor(() => expect(result.current).toEqual({ status: 'off' }))
  })

  it('keeps today’s view for a core with no live editor, an error, or an older server', async () => {
    // A scope is one object for a server's life (a new one every render
    // would be a new server every render).
    const current = scopeWith(['docs-cores-v1'])
    const older = scopeWith([])
    h.file = { core: 'board', on: true, liveView: false }
    const a = renderHook(() => useLiveDocCore(current, '/w/plan.k5board'))
    await waitFor(() => expect(h.gets.length).toBe(1))
    await waitFor(() => expect(a.result.current).toEqual({ status: 'off' }))

    h.fail = true
    const b = renderHook(() => useLiveDocCore(current, '/w/notes.md'))
    // By path, not a count: under a full parallel run the total came out 3.
    await waitFor(() => expect(h.gets.some((g) => (g.params as { path?: string }).path === '/w/notes.md')).toBe(true))
    await waitFor(() => expect(b.result.current).toEqual({ status: 'off' }))

    a.unmount()
    b.unmount()
    h.fail = false
    h.gets = []
    const c = renderHook(() => useLiveDocCore(older, '/w/notes.md'))
    expect(c.result.current).toEqual({ status: 'off' })
    expect(h.gets).toEqual([])
  })
})
