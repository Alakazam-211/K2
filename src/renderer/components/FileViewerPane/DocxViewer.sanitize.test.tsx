// @vitest-environment jsdom

// mammoth does not sanitize: a .docx hyperlink keeps whatever target the
// author typed. These tests pin the DocxViewer sanitize path so a
// `javascript:` link or an event-handler attribute can never reach the
// K2 webview.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, cleanup, waitFor, fireEvent } from '@testing-library/react'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
}))
vi.mock('@/components/Room/RoomContext', () => ({
  useRoom: () => ({ scope: 'test-scope' }),
}))
const convertToHtml = vi.fn()
vi.mock('mammoth', () => ({
  default: { convertToHtml: (...args: unknown[]) => convertToHtml(...args) },
}))
const openOffOriginHttp = vi.fn()
vi.mock('@/lib/open-off-origin-http', () => ({
  openOffOriginHttp: (...args: unknown[]) => openOffOriginHttp(...args),
}))
const openUrl = vi.fn(async (..._args: unknown[]) => undefined)
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: (...args: unknown[]) => openUrl(...args),
}))

import { sanitizeDocxHtml } from './docxSanitize'
import { DocxViewer } from './DocxViewer'

const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

function parse(html: string): HTMLElement {
  const div = document.createElement('div')
  div.innerHTML = html
  return div
}

describe('sanitizeDocxHtml', () => {
  it('strips a javascript: link target but keeps the link text', () => {
    const out = parse(sanitizeDocxHtml('<p><a href="javascript:alert(1)">click me</a></p>'))
    const a = out.querySelector('a')
    expect(a).not.toBeNull()
    expect(a!.hasAttribute('href')).toBe(false)
    expect(out.textContent).toBe('click me')
    expect(out.innerHTML).not.toMatch(/javascript/i)
  })

  it('strips obfuscated javascript: links (case, whitespace, entities)', () => {
    for (const href of ['JaVaScRiPt:alert(1)', ' javascript:alert(1)', 'jav&#x09;ascript:alert(1)', 'vbscript:x', 'data:text/html,<script>alert(1)</script>', 'file:///etc/passwd']) {
      const out = parse(sanitizeDocxHtml(`<a href="${href}">x</a>`))
      expect(out.querySelector('a')!.hasAttribute('href'), href).toBe(false)
    }
  })

  it('strips an onerror img and every other on* attribute', () => {
    const out = parse(
      sanitizeDocxHtml('<img src="x" onerror="alert(1)"><p onclick="alert(2)" onmouseover="alert(3)">t</p>'),
    )
    expect(out.innerHTML).not.toMatch(/onerror|onclick|onmouseover|alert/i)
    const img = out.querySelector('img')
    // A non-data src is dropped too, so nothing remote ever loads.
    expect(img).not.toBeNull()
    expect(img!.attributes).toHaveLength(0)
  })

  it('keeps a normal https link', () => {
    const out = parse(sanitizeDocxHtml('<p><a href="https://k2.dev/docs?a=1#b">docs</a></p>'))
    expect(out.querySelector('a')!.getAttribute('href')).toBe('https://k2.dev/docs?a=1#b')
  })

  it('keeps http, mailto and in-document fragment links', () => {
    const out = parse(
      sanitizeDocxHtml('<a href="http://example.com/">a</a><a href="mailto:hi@k2.dev">b</a><a href="#footnote-1">c</a>'),
    )
    expect(Array.from(out.querySelectorAll('a')).map((a) => a.getAttribute('href'))).toEqual([
      'http://example.com/',
      'mailto:hi@k2.dev',
      '#footnote-1',
    ])
  })

  it('keeps a data: image (mammoth inlines images)', () => {
    const out = parse(sanitizeDocxHtml(`<p><img src="${PNG}" alt="logo"></p>`))
    const img = out.querySelector('img')
    expect(img).not.toBeNull()
    expect(img!.getAttribute('src')).toBe(PNG)
    expect(img!.getAttribute('alt')).toBe('logo')
  })

  it('drops remote image sources', () => {
    const out = parse(sanitizeDocxHtml('<img src="https://tracker.example/pixel.gif">'))
    const img = out.querySelector('img')
    expect(img).not.toBeNull()
    expect(img!.hasAttribute('src')).toBe(false)
  })

  it('removes script, iframe, object, embed, style and form tags', () => {
    const out = parse(
      sanitizeDocxHtml(
        '<p>keep</p><script>alert(1)</script><iframe src="https://x"></iframe><object data="x"></object>' +
          '<embed src="x"><style>body{display:none}</style><form action="https://x"><input name="p"></form>',
      ),
    )
    expect(out.querySelectorAll('script, iframe, object, embed, style, form, input')).toHaveLength(0)
    expect(out.textContent).toBe('keep')
  })

  it('drops inline style attributes', () => {
    const out = parse(sanitizeDocxHtml('<p style="position:fixed;inset:0">x</p>'))
    expect(out.querySelector('p')!.hasAttribute('style')).toBe(false)
  })
})

describe('DocxViewer renders sanitized HTML', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    convertToHtml.mockReset()
    openOffOriginHttp.mockReset()
    openUrl.mockClear()
  })
  afterEach(() => cleanup())

  it('runs mammoth output through the sanitizer and opens links outside the webview', async () => {
    daemonCliGet.mockResolvedValue({ base64: 'UEsDBA==' })
    convertToHtml.mockResolvedValue({
      value:
        '<p><a href="javascript:alert(document.domain)">evil</a></p>' +
        '<p><img src="x" onerror="alert(1)"></p>' +
        '<p><a href="https://k2.dev/">site</a></p>' +
        `<p><img src="${PNG}" alt="pic"></p>` +
        '<p><a href="mailto:hi@k2.dev">mail</a></p>',
      messages: [],
    })

    const { container } = render(<DocxViewer filePath="/w/doc.docx" />)
    await waitFor(() => expect(container.querySelector('.docx-body')).not.toBeNull())
    const body = container.querySelector('.docx-body') as HTMLElement

    expect(daemonCliGet).toHaveBeenCalledWith('test-scope', 'fs/read-binary', { path: '/w/doc.docx' })
    expect(body.innerHTML).not.toMatch(/javascript:|onerror|alert/i)
    const links = Array.from(body.querySelectorAll('a'))
    expect(links.map((a) => a.getAttribute('href'))).toEqual([null, 'https://k2.dev/', 'mailto:hi@k2.dev'])
    const imgs = Array.from(body.querySelectorAll('img'))
    expect(imgs.filter((i) => i.getAttribute('src') === PNG)).toHaveLength(1)
    expect(imgs.filter((i) => i.hasAttribute('src') && i.getAttribute('src') !== PNG)).toHaveLength(0)

    // https: opened the app's way, and the webview itself does not navigate.
    const https = links[1]
    const notCancelled = fireEvent.click(https)
    expect(notCancelled).toBe(false)
    expect(openOffOriginHttp).toHaveBeenCalledWith('https://k2.dev/')

    const mail = links[2]
    expect(fireEvent.click(mail)).toBe(false)
    expect(openUrl).toHaveBeenCalledWith('mailto:hi@k2.dev')

    // The neutered link opens nothing.
    openOffOriginHttp.mockReset()
    openUrl.mockClear()
    fireEvent.click(links[0])
    expect(openOffOriginHttp).not.toHaveBeenCalled()
    expect(openUrl).not.toHaveBeenCalled()
  })
})
