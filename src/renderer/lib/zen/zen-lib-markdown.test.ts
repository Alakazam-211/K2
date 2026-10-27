// @vitest-environment jsdom
//
// k2-markdown@1 (prd-widget-size-and-shared-libs-v1 WL13, vs-live L12):
// the real checked-in files (marked@18, dompurify@3, k2-markdown@1) run in
// jsdom in K2's load order. The XSS cases are the security review: none
// may leave an element or attribute that runs code, and no href ever.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { ZEN_LIB_MANIFEST, zenLibResolve } from './zen-lib-loader'

const LIB_DIR = resolve(__dirname, '../../zen-lib')

function keyOf(id: string): string {
  const r = zenLibResolve([`${id}@1`])
  const k = r.keys.find((x) => typeof x === 'string' && x.startsWith(`${id}@`))
  if (typeof k !== 'string') throw new Error(`no ${id}`)
  return k
}

function load(key: string): string {
  const [id, version] = [key.slice(0, key.lastIndexOf('@')), key.slice(key.lastIndexOf('@') + 1)]
  const e = ZEN_LIB_MANIFEST.libs.find((l) => l.id === id && l.version === version)
  if (!e) throw new Error(`no row ${key}`)
  return e.files.map((f) => readFileSync(resolve(LIB_DIR, key, f.name), 'utf8')).join('\n;\n')
}

type MD = {
  render(text: string, opts?: { links?: string; images?: string }): DocumentFragment
  into(el: Element, text: string, opts?: { links?: string; images?: string }): Element
  available(): boolean
}

let K2Markdown: MD

beforeAll(() => {
  // K2's own resolver says what loads, in order: marked, DOMPurify, then k2-markdown.
  const r = zenLibResolve(['k2-markdown@1'])
  expect(r.findings.filter((f) => f.error)).toEqual([])
  expect(r.keys.map((k) => String(k).split('@')[0])).toEqual(['marked', 'dompurify', 'k2-markdown'])
  for (const k of r.keys) (0, eval)(load(String(k)))
  K2Markdown = (window as unknown as { K2Markdown: MD }).K2Markdown
  expect(K2Markdown.available()).toBe(true)
})

function html(text: string, opts?: { links?: string; images?: string }): HTMLElement {
  const el = document.createElement('div')
  K2Markdown.into(el, text, opts)
  return el
}

function runnable(el: HTMLElement): string[] {
  const bad: string[] = []
  for (const n of Array.from(el.querySelectorAll('*'))) {
    const tag = n.tagName.toLowerCase()
    if (['script', 'iframe', 'object', 'embed', 'svg', 'math', 'style', 'form', 'input', 'a', 'link', 'meta', 'base'].includes(tag)) bad.push(tag)
    for (const a of Array.from(n.attributes)) {
      if (/^on/i.test(a.name) || a.name === 'href' || a.name === 'xlink:href' || a.name === 'style' || a.name === 'srcdoc' || a.name === 'action') {
        bad.push(`${tag}[${a.name}]`)
      }
      if (a.name === 'src') bad.push(`${tag}[src]`)
    }
  }
  return bad
}

describe('k2-markdown@1: renders markdown as a fragment', () => {
  it('headings, lists, code, tables', () => {
    const el = html('# Title\n\n- one\n- **two**\n\n```\ncode\n```\n\n| a | b |\n|--|--|\n| 1 | 2 |')
    expect(el.querySelector('h1')?.textContent).toBe('Title')
    expect(el.querySelectorAll('li')).toHaveLength(2)
    expect(el.querySelector('pre code')?.textContent).toContain('code')
    expect(el.querySelector('table td')?.textContent).toBe('1')
    expect(el.classList.contains('k2md')).toBe(true)
    expect(K2Markdown.render('x')).toBeInstanceOf(DocumentFragment)
  })
})

describe('k2-markdown@1: XSS cases leave nothing that runs', () => {
  const CASES: Array<[string, string]> = [
    ['img onerror', '<img src=x onerror="alert(1)">'],
    ['script tag', '<script>alert(1)</script>'],
    ['javascript: link', '[x](javascript:alert(1))'],
    ['data:text/html link', '<a href="data:text/html,<script>alert(1)</script>">x</a>'],
    ['markdown data: link', '[x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)'],
    ['svg onload', '<svg onload=alert(1)><circle r=4></circle></svg>'],
    ['iframe', '<iframe src="https://example.test"></iframe>'],
    ['style block', '<style>body{background:url(javascript:alert(1))}</style>'],
    ['entities that decode to tags', '&lt;img src=x onerror=alert(1)&gt; &#60;script&#62;alert(1)&#60;/script&#62;'],
    ['autolink', '<javascript:alert(1)> https://example.test/x'],
    ['image with a URL', '![x](https://example.test/x.png)'],
    ['an svg data image', '![x](data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=)'],
    ['html in a table cell', '| a |\n|--|\n| <b onmouseover=alert(1)>x</b> |'],
  ]
  it.each(CASES)('%s', (_name, text) => {
    for (const opts of [undefined, { images: 'data' }, { links: 'open' }]) {
      const el = html(text, opts)
      expect(runnable(el), `${text} ${JSON.stringify(opts)}: ${el.innerHTML}`).toEqual([])
    }
  })

  it('raw HTML shows as text, and a URL stays visible as text', () => {
    const el = html('<b>bold?</b> [site](https://example.test/a)')
    expect(el.textContent).toContain('<b>bold?</b>')
    expect(el.querySelector('span.md-url')?.textContent).toBe('https://example.test/a')
  })

  it('a widget changing DOMPurify after load never loosens k2-markdown', () => {
    const P = (window as unknown as { DOMPurify: { setConfig(c: object): void; addHook(n: string, f: (n: Element, d: { keepAttr: boolean }) => void): void } }).DOMPurify
    P.setConfig({ ALLOWED_TAGS: ['a', 'img', 'script'], ALLOWED_ATTR: ['href', 'src', 'onerror'] })
    P.addHook('uponSanitizeAttribute', (_n, d) => {
      d.keepAttr = true
    })
    const el = html('<img src=x onerror="alert(1)"> [x](javascript:alert(1))')
    expect(runnable(el)).toEqual([])
  })
})

describe('k2-markdown@1: links and images', () => {
  it("links: 'open' without links:open stays text; with it, a button that calls k2.open", () => {
    const w = window as unknown as { k2?: { can(v: string): boolean; open: (u: string) => void } }
    w.k2 = { can: () => false, open: vi.fn() }
    let el = html('[docs](https://example.test/d)', { links: 'open' })
    expect(el.querySelector('button')).toBeNull()
    expect(el.querySelector('span.md-url')?.textContent).toBe('https://example.test/d')
    const open = vi.fn()
    w.k2 = { can: (v) => v === 'links.open', open }
    el = html('[docs](https://example.test/d)', { links: 'open' })
    const b = el.querySelector('button.md-url') as HTMLButtonElement
    expect(b.textContent).toBe('https://example.test/d')
    expect(b.getAttribute('href')).toBeNull()
    b.click()
    expect(open).toHaveBeenCalledWith('https://example.test/d')
    // Without opts.links, never a button, whatever the cap.
    expect(html('[docs](https://example.test/d)').querySelector('button')).toBeNull()
    delete w.k2
  })

  it("images: off by default; 'data' allows data: PNG/JPEG/GIF/WebP only", () => {
    const png = 'data:image/png;base64,iVBORw0KGgo='
    expect(html(`![dot](${png})`).querySelector('img')).toBeNull()
    const img = html(`![dot](${png})`, { images: 'data' }).querySelector('img')
    expect(img?.getAttribute('src')).toBe(png)
    expect(Array.from(img?.attributes ?? []).map((a) => a.name).sort()).toEqual(['alt', 'class', 'src'])
    expect(html('![x](data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=)', { images: 'data' }).querySelector('img')).toBeNull()
    expect(html('<img src="data:image/png;base64,iVBORw0KGgo=" onerror=alert(1)>', { images: 'data' }).querySelector('img')).toBeNull()
  })

  it('is at most 8 KB and in K2’s NOTICE', () => {
    const row = ZEN_LIB_MANIFEST.libs.find((l) => l.id === 'k2-markdown')
    expect(row?.needs).toEqual(['marked@18', 'dompurify@3'])
    expect(row!.files.reduce((n, f) => n + f.bytes, 0)).toBeLessThanOrEqual(8 * 1024)
    expect(keyOf('k2-markdown')).toBe('k2-markdown@1.0.0')
    expect(readFileSync(resolve(__dirname, '../../../../NOTICE.md'), 'utf8')).toContain('(`k2-markdown`) | 1.0.0 | FSL-1.1-Apache-2.0')
  })
})
