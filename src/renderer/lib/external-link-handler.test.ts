// @vitest-environment jsdom

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest'

const openOffOriginHttp = vi.fn()
vi.mock('@/lib/open-off-origin-http', () => ({
  openOffOriginHttp: (...args: unknown[]) => openOffOriginHttp(...args),
}))

const openUrl = vi.fn(async () => {})
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: (...args: unknown[]) => openUrl(...args),
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
})
