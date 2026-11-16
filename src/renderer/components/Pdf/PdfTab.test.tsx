// @vitest-environment jsdom
//
// The PDF tab, P1 (single player): a CAD export's layers, its existing
// annotations, text selection and Find, and the side panel's three tabs.
// Real pdf.js reads the fixture (test-utils/pdf-fixtures.ts); only drawing
// is faked (test-utils/pdfjs-fake-layers.ts), and each draw records the
// layers it was given.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...actual, daemonCliGet: (...args: unknown[]) => daemonCliGet(...args) }
})
vi.mock('pdfjs-dist', () => import('@/test-utils/pdfjs-fake-layers'))

import { PDFViewer } from '@/components/FileViewerPane/PDFViewer'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { drawingPdf, namesOnlyPdf, plainPdf, toBase64 } from '@/test-utils/pdf-fixtures'
import { holdOperatorLists, renders, resetRenders } from '@/test-utils/pdfjs-fake-layers'

async function mount(bytes = drawingPdf()): Promise<void> {
  daemonCliGet.mockResolvedValue({ base64: toBase64(bytes) })
  renderInRoom(testRoom({ tabs: {} }), <PDFViewer filePath="/work/A-101 plans.pdf" />)
  await screen.findByTestId('pdf-tab', undefined, { timeout: 5000 })
  await waitFor(() => expect(mainRenders().length).toBeGreaterThan(0))
}

const mainRenders = () => renders.filter((r) => r.canvas?.dataset.testid === 'pdf-canvas')
const lastDraw = () => mainRenders().at(-1)!
const pageShown = (): string => screen.getByTestId('pdf-page-of').textContent ?? ''

async function openPanel(tab: 'Layers' | 'Annotations' | 'Pages'): Promise<HTMLElement> {
  const toggle = screen.getByTestId('lv-panel-toggle')
  if (toggle.getAttribute('aria-pressed') !== 'true') fireEvent.click(toggle)
  fireEvent.click(await screen.findByRole('tab', { name: new RegExp(`^${tab}`) }))
  return screen.getByRole('tabpanel')
}

/** jsdom lays nothing out: the tab reports one box, so it takes keys. */
function layOut(): void {
  const real = HTMLElement.prototype.getClientRects
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function (this: HTMLElement) {
    if (this.closest('[data-testid="pdf-tab"]') === null) return real.call(this)
    return [new DOMRect(0, 0, 1200, 800)] as unknown as DOMRectList
  })
}

beforeEach(() => {
  resetRenders()
  layOut()
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  daemonCliGet.mockReset()
})

describe('the header and status line', () => {
  it('names the file once, pages it, and says page, zoom and layers below', async () => {
    await mount()
    const head = screen.getByTestId('lv-shell-header')
    expect(within(head).getByText('A-101 plans.pdf').getAttribute('title')).toBe('/work/A-101 plans.pdf')
    expect(pageShown()).toBe('1 / 2')
    const status = screen.getByTestId('editor-status')
    expect(within(status).getByText('Page 1 of 2')).toBeTruthy()
    expect(within(status).getByText('100%')).toBeTruthy()
    expect(within(status).getByText('3 of 4 layers')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Next page' }))
    await waitFor(() => expect(pageShown()).toBe('2 / 2'))
    expect(within(status).getByText('Page 2 of 2')).toBeTruthy()
  })
})

describe('layers', () => {
  it('starts from the PDF’s default config and re-renders when one is switched', async () => {
    await mount()
    expect(lastDraw().layers).toEqual({ 'A-WALL': true, 'E-LITE': true, 'A-ANNO-DIMS': false, 'M-HVAC': true })
    const panel = await openPanel('Layers')
    expect(within(panel).getByTestId('pdf-layers-count').textContent).toBe('3 of 4 visible')
    const wall = within(panel).getByRole('switch', { name: 'A-WALL' })
    expect(wall.getAttribute('aria-checked')).toBe('true')
    const before = mainRenders().length

    fireEvent.click(wall)
    await waitFor(() => expect(mainRenders().length).toBeGreaterThan(before))
    expect(lastDraw().layers['A-WALL']).toBe(false)
    expect(wall.getAttribute('aria-checked')).toBe('false')
    expect(within(panel).getByTestId('pdf-layers-count').textContent).toBe('2 of 4 visible')
    expect(within(screen.getByTestId('editor-status')).getByText('2 of 4 layers')).toBeTruthy()

    fireEvent.click(within(panel).getByRole('button', { name: 'Hide all' }))
    await waitFor(() => expect(Object.values(lastDraw().layers)).toEqual([false, false, false, false]))
    expect(within(panel).getByTestId('pdf-layers-count').textContent).toBe('0 of 4 visible')

    fireEvent.click(within(panel).getByRole('button', { name: 'Show all' }))
    await waitFor(() => expect(Object.values(lastDraw().layers)).toEqual([true, true, true, true]))
    expect(within(panel).getByTestId('pdf-layers-count').textContent).toBe('4 of 4 visible')
  })

  it('lists layers that cover nothing in one closed group, switches off, once every page is checked', async () => {
    const release = holdOperatorLists()
    await mount(drawingPdf({ unusedLayers: ['X-OLD-1', 'X-OLD-2'] }))
    const panel = await openPanel('Layers')
    expect(within(panel).getByTestId('pdf-layers-checking').textContent).toBe('Checking layers… 0 of 2 pages')
    expect(within(panel).getAllByRole('switch')).toHaveLength(6)
    expect(within(panel).queryByTestId('pdf-layers-empty')).toBeNull()

    release()
    const group = await within(panel).findByRole('button', { name: '2 layers with nothing on them' })
    expect(within(panel).queryByTestId('pdf-layers-checking')).toBeNull()
    expect(within(panel).getAllByRole('switch').map((s) => s.getAttribute('aria-label'))).toEqual(['A-WALL', 'E-LITE', 'A-ANNO-DIMS', 'M-HVAC'])
    expect(within(panel).getAllByTestId('pdf-layer-items').map((s) => s.textContent)).toEqual(['1 item', '1 item', '1 item', '1 item'])
    expect(group.getAttribute('aria-expanded')).toBe('false')
    expect(within(panel).getByTestId('pdf-layers-count').textContent).toBe('3 of 4 visible')

    fireEvent.click(group)
    expect(group.getAttribute('aria-expanded')).toBe('true')
    const empty = within(within(panel).getByTestId('pdf-layers-empty')).getAllByTestId('pdf-layer-row')
    expect(empty.map((r) => [r.textContent, r.getAttribute('title')])).toEqual([
      ['X-OLD-1', 'Nothing on this layer'],
      ['X-OLD-2', 'Nothing on this layer'],
    ])
    for (const r of empty) expect(within(r).getByRole('switch').hasAttribute('disabled')).toBe(true)
  })

  it('leads with one plain line when no layer is attached to anything', async () => {
    await mount(namesOnlyPdf(5))
    const panel = await openPanel('Layers')
    await within(panel).findByRole('button', { name: '5 layers with nothing on them' })
    expect(within(panel).getByTestId('pdf-layers-none').textContent).toBe('This file names 5 layers, but none is attached to anything on the sheets, so there’s nothing to switch.')
    expect(within(panel).queryAllByRole('switch')).toHaveLength(0)
    expect(within(screen.getByTestId('editor-status')).getByText('No layers in use')).toBeTruthy()
  })

  it('says so in one line when the PDF has none', async () => {
    await mount(plainPdf())
    const panel = await openPanel('Layers')
    expect(panel.textContent).toBe('This PDF has no layers.')
  })
})

describe('annotations on the page', () => {
  it('a comment shows its author, date and text in a popover', async () => {
    await mount()
    const section = await waitFor(() => {
      const s = document.querySelector<HTMLElement>('[data-testid="pdf-annotation-layer"] section[data-annotation-id="12R"]')
      if (!s) throw new Error('no comment section yet')
      return s
    })
    fireEvent.click(section)
    const pop = await screen.findByTestId('pdf-annot-popover')
    expect(within(pop).getByText('Sam Reyes')).toBeTruthy()
    expect(within(pop).getByText('Check the door swing at D1.')).toBeTruthy()
    expect(pop.querySelector('time')?.getAttribute('datetime')).toBe('2026-11-12T09:30:00.000Z')
    expect(within(pop).getByText('Comment')).toBeTruthy()
    fireEvent.keyDown(pop, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('pdf-annot-popover')).toBeNull())
  })

  it('an author-less comment says Unknown author', async () => {
    await mount()
    const s = await waitFor(() => document.querySelector<HTMLElement>('section[data-annotation-id="22R"]') ?? Promise.reject(new Error('not yet')))
    fireEvent.click(s)
    expect(within(await screen.findByTestId('pdf-annot-popover')).getByText('Unknown author')).toBeTruthy()
  })

  it('an internal link jumps to its page; an external one is a plain link for the app’s opener', async () => {
    await mount()
    const ext = await waitFor(() => document.querySelector<HTMLAnchorElement>('section[data-annotation-id="24R"] a') ?? Promise.reject(new Error('not yet')))
    expect(ext.getAttribute('href')).toBe('https://example.com/spec')
    expect(ext.getAttribute('target')).toBe('_blank')
    expect(ext.getAttribute('rel')).toBe('noopener noreferrer nofollow')
    const internal = document.querySelector<HTMLAnchorElement>('section[data-annotation-id="23R"] a')!
    expect(internal.getAttribute('href')?.startsWith('#')).toBe(true)
    fireEvent.click(internal)
    await waitFor(() => expect(pageShown()).toBe('2 / 2'))
    expect(screen.queryByTestId('pdf-annot-popover')).toBeNull()
  })
})

describe('the Annotations list', () => {
  it('groups by page with authors, filters by kind, and a row jumps to its page', async () => {
    await mount()
    const panel = await openPanel('Annotations')
    await waitFor(() => expect(within(panel).getAllByTestId('pdf-annot-row')).toHaveLength(7))
    expect(within(panel).getAllByRole('heading', { level: 4 }).map((h) => h.textContent)).toEqual(['Page 1', 'Page 2'])
    const rows = within(panel).getAllByTestId('pdf-annot-row')
    expect(rows[0].textContent).toContain('Sam Reyes')
    expect(rows[0].getAttribute('data-key')).toBe('c-001')
    expect(rows[1].getAttribute('data-key')).toBe('p1-2')

    fireEvent.click(within(panel).getByRole('radio', { name: /^Revision cloud/ }))
    expect(within(panel).getAllByTestId('pdf-annot-row').map((r) => r.getAttribute('data-key'))).toEqual(['rev-cloud-2'])
    fireEvent.click(within(panel).getByRole('radio', { name: /^All/ }))

    const ink = within(panel).getAllByTestId('pdf-annot-row').find((r) => r.getAttribute('data-key') === 'p2-1')!
    fireEvent.click(ink)
    await waitFor(() => expect(pageShown()).toBe('2 / 2'))
    const pop = await screen.findByTestId('pdf-annot-popover')
    expect(within(pop).getByText('Mark-up from the site visit.')).toBeTruthy()
    expect(pop.querySelector('time')?.getAttribute('datetime')).toBe('2026-11-11T16:00:00.000Z')
  })
})

describe('Pages', () => {
  it('shows a thumbnail per page; one jumps there', async () => {
    await mount()
    const panel = await openPanel('Pages')
    const thumbs = within(panel).getAllByRole('button', { name: /^Page \d/ })
    expect(thumbs.map((t) => t.getAttribute('aria-label'))).toEqual(['Page 1', 'Page 2'])
    expect(thumbs[0].getAttribute('aria-current')).toBe('page')
    await waitFor(() => expect(renders.filter((r) => r.canvas?.dataset.pdfThumb).map((r) => r.page).sort()).toEqual([1, 2]))
    fireEvent.click(thumbs[1])
    await waitFor(() => expect(pageShown()).toBe('2 / 2'))
  })
})

describe('Find', () => {
  it('counts matches across the document and steps through them, marking the current one', async () => {
    await mount()
    act(() => void fireEvent.keyDown(document.body, { key: 'f', metaKey: true }))
    const input = await screen.findByRole('searchbox', { name: 'Find' })
    expect(document.activeElement).toBe(input)
    fireEvent.change(input, { target: { value: 'door' } })
    const count = await screen.findByTestId('pdf-find-count')
    await waitFor(() => expect(count.textContent).toBe('1 of 4'))
    await waitFor(() => expect(document.querySelectorAll('[data-testid="pdf-text-layer"] .pdf-hl')).toHaveLength(3))
    expect(document.querySelectorAll('[data-testid="pdf-text-layer"] .pdf-hl[data-current]')).toHaveLength(1)

    fireEvent.keyDown(input, { key: 'Enter' })
    expect(count.textContent).toBe('2 of 4')
    fireEvent.keyDown(input, { key: 'Enter' })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(count.textContent).toBe('4 of 4')
    await waitFor(() => expect(pageShown()).toBe('2 / 2'))
    await waitFor(() => expect(document.querySelectorAll('[data-testid="pdf-text-layer"] .pdf-hl[data-current]')).toHaveLength(1))

    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    expect(count.textContent).toBe('3 of 4')
    await waitFor(() => expect(pageShown()).toBe('1 / 2'))

    fireEvent.click(screen.getByRole('button', { name: 'Match case' }))
    await waitFor(() => expect(count.textContent).toBe('No matches'))
    expect(screen.getByRole('button', { name: 'Next match' }).getAttribute('aria-disabled')).toBe('true')

    fireEvent.keyDown(input, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByRole('searchbox', { name: 'Find' })).toBeNull())
    expect(document.querySelectorAll('.pdf-hl')).toHaveLength(0)
  })
})
