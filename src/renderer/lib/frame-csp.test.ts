import { describe, expect, it } from 'vitest'
import {
  frameCsp,
  frameCspMeta,
  frameSandbox,
  frameSrcDoc,
  type FrameProfile,
} from './frame-csp'

const PROFILES: readonly FrameProfile[] = ['scripted', 'inert']

/** Parse a CSP string into directive → source list. Throws on duplicates. */
function parseCsp(csp: string): Map<string, string[]> {
  const out = new Map<string, string[]>()
  for (const raw of csp.split(';')) {
    const part = raw.trim()
    if (!part) continue
    const [name, ...sources] = part.split(/\s+/)
    if (out.has(name)) throw new Error(`duplicate directive ${name} in ${csp}`)
    out.set(name, sources)
  }
  return out
}

/** CSP3 fallback chain for the fetch directives this suite cares about. */
const FALLBACK: Record<string, string[]> = {
  'connect-src': ['default-src'],
  'img-src': ['default-src'],
  'font-src': ['default-src'],
  'media-src': ['default-src'],
  'style-src': ['default-src'],
  'script-src': ['default-src'],
  'object-src': ['default-src'],
  'manifest-src': ['default-src'],
  'frame-src': ['child-src', 'default-src'],
  'worker-src': ['child-src', 'script-src', 'default-src'],
  // Not fetch directives: no fallback. A missing one means "allow all".
  'form-action': [],
  'base-uri': [],
}

function effective(policy: Map<string, string[]>, directive: string): string[] | null {
  const own = policy.get(directive)
  if (own) return own
  const chain = FALLBACK[directive]
  if (!chain) throw new Error(`no fallback chain for ${directive}`)
  for (const d of chain) {
    const v = policy.get(d)
    if (v) return v
  }
  return null // directive absent → unrestricted
}

/**
 * Does a source list allow a URL? Supports the source forms CSP allows; a
 * frame has an opaque origin, so `'self'` matches nothing useful and is
 * treated as a failure here (the helper must never emit it).
 */
function allowsUrl(sources: string[] | null, url: string): boolean {
  if (sources === null) return true
  const u = new URL(url)
  const scheme = u.protocol.replace(/:$/, '')
  for (const src of sources) {
    if (src === "'none'") continue
    if (src === "'self'") throw new Error("frame CSP must not use 'self'")
    if (src === '*') return true
    if (src === `${scheme}:`) return true
    if (/^[a-z][a-z0-9+.-]*:$/.test(src)) continue // another scheme
    if (src.startsWith("'")) continue // keyword / nonce / hash
    const m = /^(?:([a-z][a-z0-9+.-]*):\/\/)?([^/:]+)(?::(\d+|\*))?/.exec(src)
    if (!m) throw new Error(`unparsed source ${src}`)
    const [, srcScheme, srcHost] = m
    if (srcScheme && srcScheme !== scheme) continue
    if (srcHost === '*' || srcHost === u.hostname) return true
    if (srcHost.startsWith('*.') && u.hostname.endsWith(srcHost.slice(1))) return true
  }
  return false
}

const LOOPBACK_AND_PRIVATE = [
  'http://127.0.0.1:38472/cli/projects/list?token=x',
  'http://127.0.0.1:1/',
  'http://localhost:5173/',
  'http://[::1]:8080/',
  'http://192.168.1.1/',
  'http://10.0.0.68:38472/boot-status',
  'http://example.com/',
  'ws://127.0.0.1:38472/grid',
  'https://127.0.0.1:38472/',
  'https://example.com/x.png',
  'ipc://localhost/plugin:fs|read_file',
  'tauri://localhost/index.html',
  'asset://localhost/Users/me/.k2/daemon.token',
]

describe('frameCsp — policy content', () => {
  for (const profile of PROFILES) {
    describe(profile, () => {
      const policy = parseCsp(frameCsp(profile))

      it('starts from default-src none and never names a host, http:, ipc:, *, or self', () => {
        expect(policy.get('default-src')).toEqual(["'none'"])
        const csp = frameCsp(profile)
        expect(csp).not.toMatch(/127\.0\.0\.1|localhost|\bhttp:|\bhttps:|\bws:|\bipc:|asset:|\*|'self'|unsafe-eval/)
      })

      it('denies loopback, private-network, plain-http and IPC for every fetch / nav directive', () => {
        for (const dir of [
          'connect-src',
          'frame-src',
          'worker-src',
          'img-src',
          'font-src',
          'media-src',
          'style-src',
          'script-src',
          'object-src',
          'manifest-src',
          'form-action',
          'base-uri',
        ]) {
          const sources = effective(policy, dir)
          expect(sources, `${profile}: ${dir} must be present (no fallback → allow-all)`).not.toBeNull()
          for (const url of LOOPBACK_AND_PRIVATE) {
            expect(allowsUrl(sources, url), `${profile}: ${dir} must deny ${url}`).toBe(false)
          }
        }
      })

      it('connect-src, frame-src, form-action and base-uri are exactly none', () => {
        expect(policy.get('connect-src')).toEqual(["'none'"])
        expect(policy.get('frame-src')).toEqual(["'none'"])
        expect(policy.get('form-action')).toEqual(["'none'"])
        expect(policy.get('base-uri')).toEqual(["'none'"])
      })

      it('keeps inline styles, data:/blob: images and media, data: fonts (what renders today)', () => {
        expect(policy.get('style-src')).toEqual(["'unsafe-inline'"])
        expect(policy.get('img-src')).toEqual(['data:', 'blob:'])
        expect(policy.get('media-src')).toEqual(['data:', 'blob:'])
        expect(policy.get('font-src')).toEqual(['data:'])
      })
    })
  }

  it('scripted allows inline scripts (roadmap-board style) and nothing else for scripts', () => {
    const policy = parseCsp(frameCsp('scripted'))
    expect(policy.get('script-src')).toEqual(["'unsafe-inline'"])
  })

  it('inert allows no scripts at all', () => {
    const policy = parseCsp(frameCsp('inert'))
    expect(policy.get('script-src')).toEqual(["'none'"])
  })
})

describe('frameSandbox', () => {
  it('scripted is allow-scripts only; inert is empty; never allow-same-origin', () => {
    expect(frameSandbox('scripted')).toBe('allow-scripts')
    expect(frameSandbox('inert')).toBe('')
    for (const p of PROFILES) {
      expect(frameSandbox(p)).not.toMatch(/allow-same-origin|allow-top-navigation|allow-popups|allow-forms/)
    }
  })
})

describe('frameSrcDoc — always emits the CSP first', () => {
  const META_RE = /<meta http-equiv="Content-Security-Policy" content="([^"]+)">/

  const cases: Record<string, string> = {
    empty: '',
    fragment: '<p>hello</p>',
    full: '<!DOCTYPE html><html lang="en"><head><title>x</title></head><body>x</body></html>',
    lowerDoctype: '<!doctype html>\n<script>1</script>',
    bomDoctype: '﻿  <!DOCTYPE html><body>x</body>',
    legacyDoctype:
      '<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><p>x</p>',
    commentFirst: '<!-- note --><!DOCTYPE html><p>x</p>',
    abruptComment: '<!--><script>fetch("http://127.0.0.1:1")</script>-->',
    ownMeta:
      '<!doctype html><head><meta http-equiv="Content-Security-Policy" content="default-src *"></head>',
    headScript: '<html><head><script>parent.postMessage(1,"*")</script></head></html>',
  }

  for (const profile of PROFILES) {
    for (const [name, html] of Object.entries(cases)) {
      it(`${profile} / ${name}: meta present, before any markup the document supplies`, () => {
        const out = frameSrcDoc(html, profile)
        const m = META_RE.exec(out)
        expect(m, `no CSP meta in ${JSON.stringify(out)}`).not.toBeNull()
        expect(m![1]).toBe(frameCsp(profile))
        const before = out.slice(0, m!.index)
        // Only an optional BOM/whitespace + one doctype may precede the meta.
        expect(before).toMatch(/^(﻿?[\t\n\f\r ]*<!doctype\b[^>]*>)?$/i)
        // The original document follows the meta unchanged.
        expect(before + out.slice(m!.index + m![0].length)).toBe(html)
      })
    }
  }

  it('keeps a leading doctype first so standards-mode docs stay standards-mode', () => {
    const out = frameSrcDoc('<!DOCTYPE html><p>x</p>', 'scripted')
    expect(out.startsWith('<!DOCTYPE html><meta http-equiv="Content-Security-Policy"')).toBe(true)
  })

  it('a comment before the doctype gets the meta prepended (never after attacker markup)', () => {
    const out = frameSrcDoc(cases.abruptComment, 'scripted')
    expect(out.startsWith(frameCspMeta('scripted'))).toBe(true)
  })

  it('roadmap-board style page keeps its inline script and styles under the scripted policy', () => {
    const page = [
      '<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">',
      '<style>:root{--bg:#0b0d10}.card{cursor:grab}</style></head>',
      '<body><div class="toolbar" style="display:flex"><button id="btnExport">Export</button></div>',
      '<script>(function(){const LS_KEY="k2-board";',
      'function load(){try{return JSON.parse(localStorage.getItem(LS_KEY)||"null")}catch(e){return null}}',
      'document.getElementById("btnExport").addEventListener("click",function(){',
      'const blob=new Blob(["x"],{type:"text/markdown"});URL.createObjectURL(blob)});load()})()</script>',
      '</body></html>',
    ].join('')
    const out = frameSrcDoc(page, 'scripted')
    const policy = parseCsp(frameCsp('scripted'))
    // Inline <script>, inline <style> and style="" attributes are all
    // governed by 'unsafe-inline'; the page uses no external URL.
    expect(policy.get('script-src')).toContain("'unsafe-inline'")
    expect(policy.get('style-src')).toContain("'unsafe-inline'")
    expect(out.indexOf('Content-Security-Policy')).toBeLessThan(out.indexOf('<script>'))
    expect(out.indexOf('Content-Security-Policy')).toBeLessThan(out.indexOf('<style>'))
    expect(out).toContain('addEventListener("click"')
    // Same page as inert: the meta forbids the script outright.
    expect(parseCsp(frameCsp('inert')).get('script-src')).toEqual(["'none'"])
  })
})

// prd-zen-user-widgets-v2 TUW3.1 (UW13, UW46, UWB14): the custom widget profile.
describe('frameCsp — widget profile', () => {
  const NONCE = 'q83vFzPq1N3V0aZ8k2LmTw'
  const policy = parseCsp(frameCsp('widget', { nonce: NONCE }))

  it('scripts: only the nonce (and wasm), never unsafe-inline or eval', () => {
    expect(policy.get('script-src')).toEqual([`'nonce-${NONCE}'`, "'wasm-unsafe-eval'"])
    expect(policy.get('script-src')).not.toContain("'unsafe-inline'")
    expect(policy.get('script-src')).not.toContain("'unsafe-eval'")
  })

  it('no network: connect-src is data: and blob: only, so loopback, LAN, ipc and the web are refused', () => {
    expect(policy.get('connect-src')).toEqual(['data:', 'blob:'])
    for (const url of LOOPBACK_AND_PRIVATE) {
      for (const d of ['connect-src', 'img-src', 'font-src', 'media-src', 'frame-src', 'object-src', 'manifest-src']) {
        expect(allowsUrl(effective(policy, d), url), `${d} ${url}`).toBe(false)
      }
    }
    expect(allowsUrl(effective(policy, 'connect-src'), 'data:text/plain,hi')).toBe(true)
  })

  it('workers only from blob:; no frames, objects, forms or base', () => {
    expect(policy.get('worker-src')).toEqual(['blob:'])
    for (const d of ['frame-src', 'child-src', 'object-src', 'manifest-src', 'form-action', 'base-uri']) {
      expect(policy.get(d), d).toEqual(["'none'"])
    }
    expect(policy.get('default-src')).toEqual(["'none'"])
  })

  it('the sandbox is exactly allow-scripts (null origin, no popups, modals, forms or top navigation)', () => {
    expect(frameSandbox('widget')).toBe('allow-scripts')
  })

  it('refuses a missing or malformed nonce (it is CSP source text)', () => {
    expect(() => frameCsp('widget')).toThrow(/nonce/)
    expect(() => frameCsp('widget', { nonce: "abc' 'unsafe-inline" })).toThrow(/nonce/)
    expect(() => frameCsp('widget', { nonce: 'short' })).toThrow(/nonce/)
  })

  it('the prelude goes after the meta and before every byte of the widget', () => {
    const out = frameSrcDoc('<!doctype html><html><head><script nonce="x">w()</script>', 'widget', {
      nonce: NONCE,
      prelude: '<script nonce="P">k2()</script>',
    })
    expect(out.startsWith(`<!doctype html>${frameCspMeta('widget', { nonce: NONCE })}<script nonce="P">k2()</script><html>`)).toBe(true)
  })
})
