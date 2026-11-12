// @vitest-environment jsdom

// A live deck's slides in the file's tab (V3): the daemon's listing and
// images, shown as object URLs; a slide that failed says so; a file with no
// previews says that.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<Record<string, unknown>>,
  listing: null as unknown,
  fail: null as null | { status: number; code: string },
}))

vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: async (_scope: unknown, route: string, params: Record<string, unknown>) => {
    expect(route).toBe('docs/preview')
    h.gets.push(params)
    if (h.fail) {
      const { DaemonHttpError } = await import('@/lib/daemon-http-error')
      throw new DaemonHttpError(`refused (${h.fail.code})`, h.fail.status, h.fail.code)
    }
    if (!params.container) return h.listing
    if (params.container === '@sld:257') return { state: 'failed', render_hash: 'bb', failed: { code: 'unsupported_part' } }
    return { state: 'ready', render_hash: 'aa', base64: btoa(`img ${String(params.width)}`), mime: 'image/webp', size: { w: 320, h: 180 } }
  },
}))

import { DeckPreviewPane } from './DeckPreviewPane'
import type { ServerScope } from '@/kessel/server-scope'

const scope = { connectionKey: 'k', id: 's1', serverSupports: () => true } as unknown as ServerScope
const urls: string[] = []

beforeEach(() => {
  h.gets = []
  h.fail = null
  h.listing = {
    ok: true,
    kind: 'deck',
    rev: 3,
    page: { w: 16, h: 9, unit: 'emu' },
    containers: [
      { key: '@sld:256', position: 1, title: 'Quarterly review', hidden: false, render_hash: 'aa', previews: { thumb: 'ready', full: 'ready' } },
      { key: '@sld:257', position: 2, title: 'Goals', hidden: false, render_hash: 'bb', previews: { thumb: 'failed', full: 'none' } },
    ],
  }
  let n = 0
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => `blob:${++n}`), revokeObjectURL: vi.fn((u: string) => urls.push(u)) }))
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  urls.length = 0
})

describe('DeckPreviewPane', () => {
  it('shows each slide’s thumb, the first slide at full width, and a slide it can’t draw', async () => {
    render(<DeckPreviewPane scope={scope} filePath="/w/talk.pptx" />)
    expect(await screen.findByAltText('Slide 1: Quarterly review', { selector: 'li img' })).toBeTruthy()
    expect(screen.getByText("can't draw")).toBeTruthy()
    await waitFor(() => expect(screen.getByTestId('deck-preview-stage')).toBeTruthy())
    expect(h.gets[0]).toEqual({ path: '/w/talk.pptx', want: 'thumb' })
    expect(h.gets.some((g) => g.container === '@sld:256' && g.width === 'thumb')).toBe(true)
    expect(h.gets.some((g) => g.container === '@sld:256' && g.width === 'full')).toBe(true)
    expect(h.gets.some((g) => g.container === '@sld:257' && g.width === 'thumb')).toBe(false)

    fireEvent.click(screen.getByText('Slide 2: Goals'))
    await waitFor(() => expect(screen.getByTestId('deck-preview-note').textContent).toMatch(/unsupported_part/))
  })

  it('says so when the daemon draws no previews of the file', async () => {
    h.fail = { status: 409, code: 'no_preview' }
    render(<DeckPreviewPane scope={scope} filePath="/w/talk.pptx" />)
    await waitFor(() => expect(screen.getByTestId('deck-preview-error').textContent).toBe('No previews for this file here.'))
  })

  it('revokes its object URLs when it goes', async () => {
    const r = render(<DeckPreviewPane scope={scope} filePath="/w/talk.pptx" />)
    await screen.findByTestId('deck-preview-stage')
    r.unmount()
    expect(urls.length).toBeGreaterThanOrEqual(2)
  })
})
