// @vitest-environment jsdom
//
// The PDF viewer pages with ←/→ from a window listener. That listener stays
// mounted while the viewer is hidden (an open PDF tab under Settings), and it
// used to take every arrow key in the window: in Settings → Tunnel's sign-in
// fields ← did nothing. A key typed into a field stays in the field, and a
// viewer that isn't showing takes no keys.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...actual, daemonCliGet: (...args: unknown[]) => daemonCliGet(...args) }
})
vi.mock('pdfjs-dist', () => {
  const page = {
    getViewport: () => ({ width: 100, height: 140 }),
    render: () => ({ promise: Promise.resolve(), cancel: () => undefined }),
  }
  const doc = { numPages: 3, getPage: async () => page }
  return {
    GlobalWorkerOptions: { workerSrc: '' },
    getDocument: () => ({ promise: Promise.resolve(doc) }),
  }
})

import { PDFViewer } from './PDFViewer'
import { Input } from '@/components/ui'
import { renderInRoom, testRoom } from '@/test-utils/room'

/** The Tunnel page's k2.dev sign-in fields, with an open PDF tab elsewhere in
 *  the window (`hidden`: the workspace under Settings is display:none). */
async function mount(opts: { pdfShowing: boolean }): Promise<{ email: HTMLInputElement; password: HTMLInputElement }> {
  renderInRoom(
    testRoom({ tabs: {} }),
    <>
      <form aria-label="Sign in to k2.dev">
        <Input type="email" aria-label="Email" defaultValue="you@example.com" />
        <Input type="password" aria-label="Password" defaultValue="secret" />
      </form>
      <div data-testid="pdf-tab" style={opts.pdfShowing ? undefined : { display: 'none' }}>
        <PDFViewer filePath="/work/manual.pdf" />
      </div>
    </>,
  )
  await waitFor(() => expect(screen.getByText('1 / 3')).toBeTruthy())
  return {
    email: screen.getByLabelText('Email') as HTMLInputElement,
    password: screen.getByLabelText('Password') as HTMLInputElement,
  }
}

/** jsdom lays nothing out, so every element reports no boxes (as a hidden
 *  one does in a browser). A showing viewer gets one box. */
function layOut(showing: boolean): void {
  const real = HTMLElement.prototype.getClientRects
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function (this: HTMLElement) {
    if (!showing || this.closest('[data-testid="pdf-tab"]') === null) return real.call(this)
    return [new DOMRect(0, 0, 400, 600)] as unknown as DOMRectList
  })
}

beforeEach(() => {
  daemonCliGet.mockResolvedValue({ base64: btoa('%PDF-1.4') })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  daemonCliGet.mockReset()
})

describe('PDF viewer arrow keys', () => {
  it('leaves ← and → to a focused sign-in field while the PDF tab is hidden under Settings', async () => {
    layOut(false)
    const { email, password } = await mount({ pdfShowing: false })
    for (const field of [email, password]) {
      field.focus()
      expect(document.activeElement).toBe(field)
      const before = field.value
      for (const key of ['ArrowLeft', 'ArrowRight', 'ArrowLeft']) {
        // fireEvent returns false when a listener called preventDefault (the
        // caret would not move).
        expect(fireEvent.keyDown(field, { key }), `${key} in ${field.getAttribute('aria-label')}`).toBe(true)
      }
      expect(field.value).toBe(before)
    }
    expect(screen.getByText('1 / 3')).toBeTruthy()
  })

  it('leaves arrows to a focused field even while the PDF is showing', async () => {
    layOut(true)
    const { email } = await mount({ pdfShowing: true })
    email.focus()
    expect(fireEvent.keyDown(email, { key: 'ArrowRight' })).toBe(true)
    expect(fireEvent.keyDown(email, { key: 'ArrowLeft' })).toBe(true)
    expect(email.value).toBe('you@example.com')
    expect(screen.getByText('1 / 3')).toBeTruthy()
  })

  it('still pages a showing PDF with → and ← when no field has focus', async () => {
    layOut(true)
    await mount({ pdfShowing: true })
    expect(fireEvent.keyDown(document.body, { key: 'ArrowRight' })).toBe(false)
    await waitFor(() => expect(screen.getByText('2 / 3')).toBeTruthy())
    expect(fireEvent.keyDown(document.body, { key: 'ArrowLeft' })).toBe(false)
    await waitFor(() => expect(screen.getByText('1 / 3')).toBeTruthy())
  })

  it('takes no keys while hidden, even with no field focused', async () => {
    layOut(false)
    await mount({ pdfShowing: false })
    expect(fireEvent.keyDown(document.body, { key: 'ArrowRight' })).toBe(true)
    expect(screen.getByText('1 / 3')).toBeTruthy()
  })
})
