// @vitest-environment jsdom
//
// An HTML doc tile on a dashboard (K2 Design, 2026-11-17: a tile drew the word
// "undefined" instead of the page). The tile drew `fs/read-file`'s `content`
// unchecked: a 200 whose body isn't `{content}` (a proxy's or sign-in page,
// which parses to a string) became `fitToTile(undefined)`, the word
// "undefined" in the frame. Now that read is the tile's error state.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({ read: null as unknown }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: async (_scope: unknown, route: string) => {
    if (route === 'fs/read-file') return h.read
    throw new Error(`unexpected route ${route}`)
  },
}))

import { NOT_FILE_TEXT } from '@/lib/read-file-text'
import { HtmlDocPane } from './ProjectDashboard'

const srcdoc = (): string => document.querySelector('iframe')?.getAttribute('srcdoc') ?? ''

beforeEach(() => {
  h.read = { content: '<!doctype html><html><head></head><body><h1>Status</h1></body></html>', path: '/w/ada/status.html', name: 'status.html' }
})
afterEach(() => cleanup())

describe('an HTML doc tile', () => {
  it('draws the page it read', async () => {
    render(<HtmlDocPane filePath="/w/ada/status.html" workspaceId="w2" />)
    await waitFor(() => expect(srcdoc()).toContain('<h1>Status</h1>'))
  })

  it('a read that is not the file’s text is the error tile, never "undefined"', async () => {
    h.read = '<html>Sign in</html>'
    render(<HtmlDocPane filePath="/w/ada/status.html" workspaceId="w2" />)
    const err = await screen.findByTestId('doc-tile-error')
    expect(err.textContent).toContain('Couldn’t open status.html')
    // The tile's detail line drops the closing period.
    expect(err.textContent).toContain(NOT_FILE_TEXT.replace(/\.$/, ''))
    expect(document.querySelector('iframe')).toBeNull()
    expect(document.body.textContent).not.toContain('undefined')
  })
})
