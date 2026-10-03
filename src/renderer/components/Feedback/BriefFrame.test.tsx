// @vitest-environment jsdom
// prd-ticket-html-brief-v1 T9/T10 (+ H13, H18, H38): the brief renders only
// through HtmlFrame's inert profile, links leave the frame and open in the
// system browser, and Expand shows the same document full-page.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { frameCsp, frameSrcDoc } from '@/lib/frame-csp'
import { BriefFrame } from './BriefFrame'
import { buildBriefSrcDoc, briefStylesheet, fallbackBriefTokens } from './brief-srcdoc'
import type { FeedbackBrief } from './feedback-api'

const opener = vi.hoisted(() => ({ openUrl: vi.fn(async () => {}) }))
vi.mock('@tauri-apps/plugin-opener', () => opener)

const offOrigin = vi.hoisted(() => ({ openOffOriginHttp: vi.fn() }))
vi.mock('@/lib/open-off-origin-http', () => offOrigin)

const CLEANED =
  '<h2>Problem</h2><p>DNS for <a href="https://example.com/docs/dns?x=1&amp;y=2" rel="noopener noreferrer">the docs</a> is stale.</p>' +
  '<section class="k2-need"><h2>What I need from you</h2><p>Pick one. Mail <a href="mailto:ops@example.com" rel="noopener noreferrer">ops</a>.</p></section>' +
  '<ol class="k2-options"><li>Retry</li><li>Hold</li></ol>' +
  '<p><img src="data:image/png;base64,iVBORw0KGgo=" alt="shot"></p>'

const brief: FeedbackBrief = {
  html: CLEANED,
  text: 'Problem DNS for the docs is stale.',
  bytes: CLEANED.length,
  sha256: 'abc123',
  sanitizer: 'k2-brief-v1',
  createdAt: 1_759_241_100,
}

afterEach(() => {
  cleanup()
  opener.openUrl.mockClear()
  offOrigin.openOffOriginHttp.mockClear()
})

describe('BriefFrame — source (T9)', () => {
  const src = readFileSync(join(__dirname, 'BriefFrame.tsx'), 'utf8').replace(
    /\/\*[\s\S]*?\*\/|\/\/.*$/gm,
    '',
  )

  it('renders through HtmlFrame with the inert profile only', () => {
    expect(src).toMatch(/import \{ HtmlFrame \} from '@\/components\/HtmlFrame\/HtmlFrame'/)
    const uses = [...src.matchAll(/<HtmlFrame\b[\s\S]*?\/>/g)].map((m) => m[0])
    expect(uses.length).toBe(2)
    for (const use of uses) expect(use).toContain('profile="inert"')
  })

  it('never loosens the frame or injects HTML into the parent', () => {
    expect(src).not.toMatch(/allow-scripts/)
    expect(src).not.toMatch(/allow-same-origin/)
    expect(src).not.toMatch(/dangerouslySetInnerHTML/)
    expect(src).not.toMatch(/<iframe/i)
    expect(src).not.toMatch(/openOffOriginHttp/)
  })
})

describe('buildBriefSrcDoc (T10)', () => {
  const tokens = fallbackBriefTokens('dark')

  it('strips every href, numbers the links, and lists the full URLs', () => {
    const { frameHtml, links } = buildBriefSrcDoc(CLEANED, tokens)
    expect(links).toEqual([
      { n: 1, url: 'https://example.com/docs/dns?x=1&y=2' },
      { n: 2, url: 'mailto:ops@example.com' },
    ])
    const doc = new DOMParser().parseFromString(frameHtml, 'text/html')
    const anchors = Array.from(doc.querySelectorAll('a'))
    expect(anchors).toHaveLength(2)
    for (const a of anchors) expect(a.hasAttribute('href')).toBe(false)
    expect(frameHtml).not.toMatch(/href=/i)
    expect(anchors.map((a) => a.getAttribute('data-k2-link'))).toEqual(['1', '2'])
    expect(anchors[0].textContent).toBe('the docs[1]')
  })

  it('wraps the brief in the K2 document with the themed stylesheet', () => {
    const { frameHtml } = buildBriefSrcDoc(CLEANED, tokens)
    expect(frameHtml.startsWith('<!doctype html><html><head><meta charset="utf-8"><style>')).toBe(true)
    expect(frameHtml).toContain(`<style>${briefStylesheet(tokens)}</style>`)
    expect(frameHtml).toContain('<body class="k2-brief">')
    expect(briefStylesheet(tokens)).toContain('color-scheme:dark')
    expect(briefStylesheet(fallbackBriefTokens('light'))).toContain('color-scheme:light')
    expect(briefStylesheet(tokens)).toContain('.k2-need{')
    expect(briefStylesheet(tokens)).not.toMatch(/background:\s*(#fff\b|#ffffff|white)/i)
    // Inline data: images survive the presentation pass untouched.
    expect(frameHtml).toContain('src="data:image/png;base64,iVBORw0KGgo="')
  })

  it('through frameSrcDoc the inert CSP meta comes right after the doctype', () => {
    const { frameHtml } = buildBriefSrcDoc(CLEANED, tokens)
    const wrapped = frameSrcDoc(frameHtml, 'inert')
    expect(wrapped.startsWith(
      `<!doctype html><meta http-equiv="Content-Security-Policy" content="${frameCsp('inert')}"><html>`,
    )).toBe(true)
    expect(frameCsp('inert')).toContain("script-src 'none'")
  })

  it('a hostile token value cannot break out of the stylesheet', () => {
    const css = briefStylesheet({ ...tokens, accent: 'red}</style><script>alert(1)</script>' })
    expect(css).not.toContain('</style>')
    expect(css).toContain(`--accent:${tokens.accent}`)
  })
})

describe('BriefFrame — rendered (T10, H13, H18)', () => {
  it('renders an inert frame whose document has no hrefs', () => {
    render(<BriefFrame brief={brief} title="Deploy blocked: DNS?" />)
    const f = screen.getByTestId('brief-frame')
    expect(f.tagName).toBe('IFRAME')
    expect(f.getAttribute('sandbox')).toBe('')
    expect(f.getAttribute('referrerpolicy')).toBe('no-referrer')
    const doc = f.getAttribute('srcdoc')
    expect(doc).not.toBeNull()
    expect(doc!).toContain(frameCsp('inert'))
    expect(doc!).not.toMatch(/href=/i)
    expect(doc!).toContain('<body class="k2-brief">')
    // Fixed default height with its own box; no auto-size.
    const box = screen.getByTestId('brief-box')
    expect(box.className).toContain('h-[min(60vh,560px)]')
    expect(box.style.height).toBe('')
  })

  it('lists the full URLs and opens them in the system browser', () => {
    render(<BriefFrame brief={brief} title="Deploy blocked: DNS?" />)
    const links = screen.getAllByTestId('brief-link')
    expect(links.map((l) => l.textContent)).toEqual([
      'https://example.com/docs/dns?x=1&y=2',
      'mailto:ops@example.com',
    ])
    fireEvent.click(links[0])
    expect(opener.openUrl).toHaveBeenCalledTimes(1)
    expect(opener.openUrl).toHaveBeenCalledWith('https://example.com/docs/dns?x=1&y=2')
    expect(offOrigin.openOffOriginHttp).not.toHaveBeenCalled()
  })

  it('the resize handle changes the frame height', () => {
    render(<BriefFrame brief={brief} title="t" />)
    const handle = screen.getByTestId('brief-resize')
    fireEvent.keyDown(handle, { key: 'ArrowDown' })
    expect(screen.getByTestId('brief-box').style.height).toMatch(/^\d+px$/)
  })

  it('Expand opens the same document full-page; Close and Escape dismiss it', () => {
    render(<BriefFrame brief={brief} title="Deploy blocked: DNS?" />)
    expect(screen.queryByTestId('brief-overlay')).toBeNull()

    fireEvent.click(screen.getByTestId('brief-expand'))
    const overlay = screen.getByTestId('brief-overlay')
    const big = screen.getByTestId('brief-overlay-frame')
    expect(big.getAttribute('sandbox')).toBe('')
    expect(big.getAttribute('srcdoc')).toBe(screen.getByTestId('brief-frame').getAttribute('srcdoc'))
    expect(overlay.parentElement).toBe(document.body)

    fireEvent.click(screen.getByTestId('brief-overlay-close'))
    expect(screen.queryByTestId('brief-overlay')).toBeNull()

    fireEvent.click(screen.getByTestId('brief-expand'))
    expect(screen.getByTestId('brief-overlay')).toBeTruthy()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByTestId('brief-overlay')).toBeNull()
  })

  it('rebuilds the document when the app scheme changes', async () => {
    document.documentElement.setAttribute('data-scheme', 'dark')
    render(<BriefFrame brief={brief} title="t" />)
    const before = screen.getByTestId('brief-frame').getAttribute('srcdoc')!
    expect(before).toContain('color-scheme:dark')
    document.documentElement.setAttribute('data-scheme', 'light')
    await screen.findByTestId('brief-frame')
    await vi.waitFor(() => {
      expect(screen.getByTestId('brief-frame').getAttribute('srcdoc')).toContain('color-scheme:light')
    })
    document.documentElement.setAttribute('data-scheme', 'dark')
  })
})
