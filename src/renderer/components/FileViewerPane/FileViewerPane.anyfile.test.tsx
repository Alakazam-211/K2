// @vitest-environment jsdom
//
// The file tab and the "Any other file" core: while the daemon says the core
// is on, the panel sits beside the tab's usual view; while it's off, nothing
// multiplayer is shown or asked for.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({ on: true, core: 'any-file', liveView: false, gets: [] as string[], statusRefusal: null as unknown }))
const events = vi.hoisted(() => ({ handlers: new Set<(e: unknown) => void>() }))

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: async (_scope: unknown, route: string) => {
      h.gets.push(route)
      switch (route) {
        case 'fs/read-file':
          return { content: 'ascii part\n' }
        case 'docs/cores':
          return { ok: true, cores: [], file: { core: h.core, on: h.on, liveView: h.liveView } }
        case 'docs/status':
          if (h.statusRefusal) throw h.statusRefusal
          return { ok: true, tier: 0, version: 1, lease: null, rights: { read: true, edit: true, history: true }, conflictCopies: [] }
        case 'docs/who':
          return { ok: true, roster: [] }
        case 'docs/versions':
          return { ok: true, versions: [] }
        case 'docs/approvals':
          return { ok: true, approvals: [] }
        default:
          throw new Error(`unexpected route ${route}`)
      }
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
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { setLivePreview } from '@/lib/live-view/shared/preview-keys'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { FileViewerPane } from './FileViewerPane'

const q = (id: string): HTMLElement | null => document.querySelector(`[data-testid="${id}"]`)
const TIER0 = ['docs/status', 'docs/who', 'docs/versions', 'docs/approvals']

beforeEach(() => {
  h.on = true
  h.core = 'any-file'
  h.liveView = false
  h.gets = []
  h.statusRefusal = null
})
afterEach(() => {
  cleanup()
  events.handlers.clear()
})

describe('file tab: any-file panel', () => {
  it('sits beside the usual view while the core is on, and the toolbar closes it', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/part.step" paneId="p" tabId="t" />)
    await waitFor(() => expect(q('any-file-panel')).not.toBeNull())
    await waitFor(() => expect(document.body.textContent).toContain('Who’s here'))
    expect(document.body.textContent).toContain('part.step')
    expect(q('file-viewer-live-badge')).toBeNull()

    const toggle = q('any-file-toggle') as HTMLElement
    expect(toggle.getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-pressed')).toBe('false')
    expect(q('any-file-panel')?.className).toBe('hidden')
  })

  it('goes away when the core is switched off', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/part.step" paneId="p" tabId="t" />)
    await waitFor(() => expect(q('any-file-panel')).not.toBeNull())
    h.on = false
    await act(async () => {
      for (const fn of events.handlers) fn({ kind: 'doc_cores_changed', core: 'any-file', on: false })
    })
    await waitFor(() => expect(q('any-file-panel')).toBeNull())
    expect(q('any-file-toggle')).toBeNull()
  })

  it('a core_off answer from the daemon makes the tab ask again', async () => {
    h.statusRefusal = new DaemonHttpError('off', 403, 'core_off')
    h.on = true
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/part.step" paneId="p" tabId="t" />)
    await waitFor(() => expect(h.gets.filter((r) => r === 'docs/cores').length).toBeGreaterThanOrEqual(2))
  })

  it('with the core off there is no panel and no tier 0 call', async () => {
    h.on = false
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/part.step" paneId="p" tabId="t" />)
    await waitFor(() => expect(h.gets).toContain('docs/cores'))
    await waitFor(() => expect(document.body.textContent).toContain('part.step'))
    expect(q('any-file-panel')).toBeNull()
    expect(q('any-file-toggle')).toBeNull()
    expect(h.gets.filter((r) => TIER0.includes(r))).toEqual([])
  })

  it('a Board core with no live editor gets no any-file panel', async () => {
    h.core = 'board'
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/part.step" paneId="p" tabId="t" />)
    await waitFor(() => expect(h.gets).toContain('docs/cores'))
    await waitFor(() => expect(document.body.textContent).toContain('part.step'))
    expect(q('any-file-panel')).toBeNull()
  })
})

describe('file tab: watching live (preview)', () => {
  afterEach(() => window.localStorage.clear())

  it('the live header replaces the file toolbar: the file is named once, and File brings the toolbar back', async () => {
    setLivePreview('scene', true)
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/living-room.blend" paneId="p" tabId="t" />)
    await waitFor(() => expect(q('live-scene-watch')).not.toBeNull())
    expect(document.querySelector('[title="Search (Cmd+F)"]')).not.toBeNull()
    fireEvent.click(q('live-scene-watch') as HTMLElement)
    await waitFor(() => expect(q('lv-shell-header')).not.toBeNull(), { timeout: 5000 })
    // No toolbar row: no search, refresh or close, and no second file name.
    expect(document.querySelector('[title="Search (Cmd+F)"]')).toBeNull()
    expect(document.querySelector('[title="Refresh"]')).toBeNull()
    expect(screen.queryByText('living-room.blend')).toBeNull()
    expect(screen.queryByText('/ws/living-room.blend')).toBeNull() // nor the path bar at the foot
    expect(screen.getAllByRole('radiogroup', { name: 'Scene view' })).toHaveLength(1)
    fireEvent.click(within(q('lv-shell-header') as HTMLElement).getByRole('radio', { name: 'File' }))
    await waitFor(() => expect(document.querySelector('[title="Search (Cmd+F)"]')).not.toBeNull())
    expect(screen.getByText('living-room.blend')).toBeTruthy()
    expect(screen.getByText('/ws/living-room.blend')).toBeTruthy()
  })
})
