// @vitest-environment jsdom
//
// An HTML document opens in the HTML preview, not as source. That holds
// with the file's multiplayer core on too (the text pack covers .html): the
// Live editor is behind Edit, and Preview brings the page back. Every
// opener (file tree, Files drawer, a Thread link, `k2 open`) lands on this
// pane, so the pane's first view is the one place that decides.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const PAGE = '<!doctype html><title>Report</title><h1>Hello</h1>'
const h = vi.hoisted(() => ({ on: false, gets: [] as string[] }))

vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: async (_scope: unknown, route: string, params: { path?: string }) => {
      h.gets.push(route)
      if (route === 'fs/read-file') return { content: '<!doctype html><title>Report</title><h1>Hello</h1>' }
      if (route === 'docs/cores') {
        if (params?.path !== '/ws/report.html') throw new Error(`docs/cores for ${params?.path}`)
        return { ok: true, cores: [], file: { core: 'text', on: h.on, liveView: true } }
      }
      throw new Error(`unexpected route ${route}`)
    },
    daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
// The frame's sandbox and CSP have their own tests; here only which view shows.
vi.mock('@/components/HtmlFrame/HtmlFrame', () => ({
  HtmlFrame: ({ html }: { html: string }) => <div data-testid="html-preview" data-html={html} />,
}))
vi.mock('./LiveDocPane', () => ({ LiveDocPane: () => <div data-testid="live-doc-pane" /> }))
vi.mock('./CodeEditor', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./CodeEditor')>()),
  CodeEditor: ({ code }: { code: string }) => <div data-testid="code-editor">{code}</div>,
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { FileViewerPane } from './FileViewerPane'

beforeEach(() => {
  h.on = false
  h.gets = []
})
afterEach(() => {
  cleanup()
})

function preview(): HTMLElement | null {
  return document.querySelector('[data-testid="html-preview"]')
}

describe('file tab: an HTML document', () => {
  it('opens in the HTML preview; Edit shows the source and Preview brings the page back', async () => {
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/report.html" paneId="p" tabId="t" />)
    await waitFor(() => expect(preview()?.getAttribute('data-html')).toBe(PAGE))
    expect(document.querySelector('[data-testid="code-editor"]')).toBeNull()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    })
    await waitFor(() => expect(document.querySelector('[data-testid="code-editor"]')?.textContent).toBe(PAGE))
    expect(preview()).toBeNull()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Preview' }))
    })
    await waitFor(() => expect(preview()).not.toBeNull())
  })

  it('with its multiplayer core on it still opens in the preview; the Live editor is behind Edit', async () => {
    h.on = true
    renderInPrimaryRoom(<FileViewerPane filePath="/ws/report.html" paneId="p" tabId="t" />)
    // The core's answer lands, then the view settles: the page, not the editor.
    await waitFor(() => expect(document.querySelector('[data-testid="file-viewer-live-badge"]')).not.toBeNull())
    await waitFor(() => expect(preview()?.getAttribute('data-html')).toBe(PAGE))
    expect(document.querySelector('[data-testid="file-viewer-live"]')).toBeNull()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    })
    await waitFor(() => expect(document.querySelector('[data-testid="live-doc-pane"]')).not.toBeNull())
    expect(preview()).toBeNull()
  })
})
