// @vitest-environment jsdom

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest'

const openOffOriginHttp = vi.fn<(href: string) => void>()
vi.mock('@/lib/open-off-origin-http', () => ({
  openOffOriginHttp: (href: string) => openOffOriginHttp(href),
}))

const openUrl = vi.fn<(url: string) => Promise<void>>()
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: (url: string) => openUrl(url),
}))

import { installExternalLinkHandler } from './external-link-handler'

function clickHref(href: string): { event: MouseEvent; anchor: HTMLAnchorElement } {
  const anchor = document.createElement('a')
  anchor.setAttribute('href', href)
  anchor.textContent = 'link'
  document.body.appendChild(anchor)
  const event = new MouseEvent('click', { bubbles: true, cancelable: true })
  anchor.dispatchEvent(event)
  return { event, anchor }
}

describe('installExternalLinkHandler', () => {
  beforeAll(() => {
    installExternalLinkHandler()
  })

  beforeEach(() => {
    document.body.innerHTML = ''
    openOffOriginHttp.mockReset()
    openUrl.mockReset()
  })

  it('preventDefault on off-origin http(s) and routes to the Browser tab', () => {
    const { event } = clickHref('http://127.0.0.1:8788')
    expect(event.defaultPrevented).toBe(true)
    expect(openOffOriginHttp).toHaveBeenCalledTimes(1)
    expect(openOffOriginHttp.mock.calls[0]?.[0]).toBe('http://127.0.0.1:8788')
  })

  it('does not preventDefault on same-document hash links', () => {
    const { event } = clickHref('#foo')
    expect(event.defaultPrevented).toBe(false)
    expect(openOffOriginHttp).toHaveBeenCalledTimes(0)
    expect(openUrl).toHaveBeenCalledTimes(0)
  })

  it('does not preventDefault on same-origin http', () => {
    const same = `${window.location.origin}/spa-path`
    const { event } = clickHref(same)
    expect(event.defaultPrevented).toBe(false)
    expect(openOffOriginHttp).toHaveBeenCalledTimes(0)
  })

  it('preventDefault on javascript: and does not open anything', () => {
    const { event } = clickHref('javascript:alert(1)')
    expect(event.defaultPrevented).toBe(true)
    expect(openOffOriginHttp).toHaveBeenCalledTimes(0)
    expect(openUrl).toHaveBeenCalledTimes(0)
  })

  it('leaves a chat markdown link (data-chat-link) to its own click handler', () => {
    const anchor = document.createElement('a')
    anchor.setAttribute('href', 'https://example.com/')
    anchor.setAttribute('data-chat-link', '')
    document.body.appendChild(anchor)
    // Stands in for ChatLink's own handler, which opens it in the default browser.
    anchor.addEventListener('click', (e) => e.preventDefault())
    anchor.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    expect(openOffOriginHttp).toHaveBeenCalledTimes(0)
    expect(openUrl).toHaveBeenCalledTimes(0)
  })
})
