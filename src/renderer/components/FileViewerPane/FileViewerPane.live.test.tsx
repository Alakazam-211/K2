// @vitest-environment jsdom
//
// The file tab and live docs (0.46.7): a file whose multiplayer core is on
// opens in the Live editor (the daemon's `docs/cores?path=` says so), with a
// Live badge and no Save; turning the core off (the switch, seen through
// `doc_cores_changed`, or the socket's `core_off` close) brings back
// today's editor, the socket close with a line saying why.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({ on: true, gets: [] as string[] }))
const events = vi.hoisted(() => ({ handlers: new Set<(e: unknown) => void>() }))
const live = vi.hoisted(() => ({ transports: [] as Array<{ end: (e: unknown) => void; sent: unknown[] }> }))

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: async (_scope: unknown, route: string, params: { path?: string }) => {
      h.gets.push(route)
      if (route === 'fs/read-file') return { content: '# Notes\n' }
      if (route === 'docs/cores') {
        if (params?.path !== '/ws/notes.md') throw new Error(`docs/cores for ${params?.path}`)
        return { ok: true, cores: [], file: { core: 'markdown', on: h.on, liveView: true } }
      }
      throw new Error(`unexpected route ${route}`)
    },
    daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocCoresChanged: (_scope: unknown, fn: (e: unknown) => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))
vi.mock('@/lib/docs-live-daemon', () => ({
  createDaemonLiveTransport: () => {
    const frameCbs = new Set<(f: unknown) => void>()
    const closeCbs = new Set<(e: unknown) => void>()
    let open = false
    const t = {
      sent: [] as unknown[],
      get open() {
        return open
      },
      async connect() {
        open = true
        return { root: 'source', version: 1, text: '# Notes\n', client: 1, self: 'person:owner', rights: 'edit' }
      },
      send(f: unknown) {
        if (open) t.sent.push(f)
      },
      onFrame(cb: (f: unknown) => void) {
        frameCbs.add(cb)
        return () => void frameCbs.delete(cb)
      },
      onClose(cb: (e: unknown) => void) {
        closeCbs.add(cb)
        return () => void closeCbs.delete(cb)
      },
      close() {
        open = false
      },
      end(e: unknown) {
        open = false
        for (const cb of closeCbs) cb(e)
      },
    }
    live.transports.push(t)
    return t
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { FileViewerPane } from './FileViewerPane'

beforeEach(() => {
  h.on = true
  h.gets = []
  live.transports = []
})
afterEach(() => {
  cleanup()
  events.handlers.clear()
})

async function liveView(): Promise<void> {
  await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live"]')).not.toBeNull())
  await waitFor(() => expect(document.querySelector('[data-testid="live-doc-view"]')?.getAttribute('data-state')).toBe('live'))
}

describe('file tab: live docs', () => {
  it('opens a file whose core is on in the Live editor, and goes back when the switch turns off', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    await liveView()
    expect(document.querySelector('[data-testid="file-viewer-live-badge"]')?.textContent).toBe('Live')
    expect(document.querySelector('.cm-content')?.textContent).toContain('# Notes')
    expect(live.transports).toHaveLength(1)

    h.on = false
    await act(async () => {
      for (const fn of events.handlers) fn({ kind: 'doc_cores_changed', core: 'markdown', on: false, by: 'Anna' })
    })
    await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live"]')).toBeNull())
    expect(document.querySelector('[data-testid="file-viewer-live-badge"]')).toBeNull()
  })

  it('a core_off close hands the tab back with a line saying why', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    await liveView()
    h.on = false
    await act(async () => live.transports[0].end({ reason: 'closed', retry: false, code: 'core_off' }))
    await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live-ended"]')).not.toBeNull())
    await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live"]')).toBeNull())
  })

  it('a core that is off keeps today’s editor', async () => {
    h.on = false
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/notes.md" paneId="p" tabId="t" />)
    await waitFor(() => expect(h.gets).toContain('docs/cores'))
    await waitFor(() => expect(document.body.textContent).toContain('notes.md'))
    expect(document.querySelector('[data-testid="file-viewer-live"]')).toBeNull()
    expect(live.transports).toHaveLength(0)
  })
})
