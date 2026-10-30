// @vitest-environment jsdom
// PV-5: a private site in a K2 tab takes no step. The tab asks the daemon (via
// `siteUrlForTab`) and loads the one-time link, while the address it keeps, and
// reopens with, stays the plain one, because a link works only once.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import { BrowserPane } from './BrowserPane'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

const swap = vi.fn()
vi.mock('@/lib/published-sites', () => ({
  siteUrlForTab: (scope: unknown, url: string) => swap(scope, url),
}))

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const PLAIN = 'https://docs.rosson.k2.dev/guide'
const LINK = 'https://docs.rosson.k2.dev/_k2/site/open?code=abc'

function renderTab(url: string): void {
  renderInPrimaryRoom(<BrowserPane itemId="b1" tabId="tab-1" paneGroupId="pg-1" url={url} standalone />)
}

describe('a private site in a K2 tab', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset()
    vi.mocked(invoke).mockImplementation(async () => null)
    swap.mockReset()
    swap.mockImplementation(async (_s: unknown, url: string) => (url === PLAIN ? LINK : url))
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 32,
      width: 640,
      height: 480,
      top: 32,
      left: 0,
      right: 640,
      bottom: 512,
      toJSON() {
        return {}
      },
    } as DOMRect)
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('opening one loads the one-time link and keeps the plain address', async () => {
    vi.useFakeTimers()
    renderTab(PLAIN)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(swap).toHaveBeenCalledWith(expect.anything(), PLAIN)
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('browser_create', expect.objectContaining({ url: LINK }))
    const address = screen.getByRole('textbox') as HTMLInputElement
    expect(address.value).toBe(PLAIN)
  })

  it('going to one from the address field does the same', async () => {
    vi.useFakeTimers()
    renderTab('https://example.com')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('browser_create', expect.objectContaining({ url: 'https://example.com' }))
    const address = screen.getByRole('textbox') as HTMLInputElement
    fireEvent.change(address, { target: { value: PLAIN } })
    fireEvent.keyDown(address, { key: 'Enter' })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('browser_navigate', expect.objectContaining({ url: LINK }))
  })
})
