// U0 (prd-garden-board-needs-v1 GS8, R4a): can a widget in K2's sealed frame
// draw a `data:` SVG with `foreignObject` onto a canvas and read it back?
// Board's PNG export does exactly that. The page here is K2's real frame
// shape: `sandbox="allow-scripts"` and the frame CSP from
// src/shared/zen-frame-shape.golden.html (img-src data: blob:), with a nonce.
//
// Recorded per engine in EXPECTED; an engine change fails the test. The real
// webviews (WKWebView on a signed build, WebKitGTK, WebView2) are measured by
// hand and noted in docs/zen-contract.md ("What a widget can draw to a canvas").
//
// Run: npx playwright test --config tests/frame/playwright.config.ts canvas-taint
import { expect, test } from '@playwright/test'

/** Per canvas (a plain one, then one drawn from the SVG):
 *  getImageData / toDataURL / toBlob, each 'ok' or what went wrong.
 *  Measured 2026-10-27 (Playwright 1.61 on macOS, k2n3-m1). */
const EXPECTED: Record<string, string> = {
  chromium: 'plain ok/ok/ok; svg ok/ok/ok',
  // WebKit doesn't taint the SVG canvas (getImageData works), but in an
  // opaque-origin sandboxed frame it encodes NO canvas at all: toDataURL
  // answers an empty data: URL and toBlob answers null, even for a plain
  // canvas. A widget encodes the PNG itself from getImageData (or K2 adds
  // a native capture, GS9).
  webkit: 'plain ok/empty/null; svg ok/empty/null',
}

const CSP =
  "default-src 'none'; script-src 'nonce-U0U0U0U0U0U0U0U0U0U0U0'; style-src 'unsafe-inline'; img-src data: blob:; " +
  "media-src data: blob:; font-src data:; connect-src data: blob:; frame-src 'none'; child-src 'none'; worker-src blob:; object-src 'none'"

const FRAME = `<!DOCTYPE html><meta http-equiv="Content-Security-Policy" content="${CSP}">
<body><script nonce="U0U0U0U0U0U0U0U0U0U0U0">
const dot = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><foreignObject width="120" height="60">' +
  '<div xmlns="http://www.w3.org/1999/xhtml" style="font:14px sans-serif">Board <img src="' + dot + '" width="8" height="8"/></div>' +
  '</foreignObject></svg>'
const img = new Image()
const send = (r) => parent.postMessage({ u0: r }, '*')
const probe = (c, g) => {
  const out = {}
  try { g.getImageData(0, 0, 1, 1); out.read = 'ok' } catch (e) { out.read = e && e.name ? e.name : String(e) }
  try { out.dataUrl = c.toDataURL('image/png').startsWith('data:image/png') ? 'ok' : 'empty' } catch (e) { out.dataUrl = e && e.name ? e.name : String(e) }
  return new Promise((done) => {
    try { c.toBlob((b) => { out.blob = b ? 'ok' : 'null'; done(out) }, 'image/png') } catch (e) { out.blob = e && e.name ? e.name : String(e); done(out) }
  })
}
img.onload = async () => {
  const plain = document.createElement('canvas')
  plain.width = 4
  plain.height = 4
  const pg = plain.getContext('2d')
  pg.fillRect(0, 0, 2, 2)
  const c = document.createElement('canvas')
  c.width = 120
  c.height = 60
  const g = c.getContext('2d')
  g.drawImage(img, 0, 0)
  const a = await probe(plain, pg)
  const b = await probe(c, g)
  send('plain ' + a.read + '/' + a.dataUrl + '/' + a.blob + '; svg ' + b.read + '/' + b.dataUrl + '/' + b.blob)
}
img.onerror = () => send('image-error')
img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg)
</script></body>`

test('a data: SVG with foreignObject on a canvas, in the sealed frame', async ({ page, browserName }) => {
  await page.setContent('<!doctype html><body></body>')
  const result = await page.evaluate(
    (srcdoc) =>
      new Promise<string>((resolve) => {
        window.addEventListener('message', (e) => {
          const d = e.data as { u0?: string }
          if (d && typeof d.u0 === 'string') resolve(d.u0)
        })
        const f = document.createElement('iframe')
        f.setAttribute('sandbox', 'allow-scripts')
        f.srcdoc = srcdoc
        document.body.appendChild(f)
      }),
    FRAME,
  )
  test.info().annotations.push({ type: 'u0', description: `${browserName}: ${result}` })
  expect(EXPECTED[browserName], `no recorded result for ${browserName}`).toBeDefined()
  expect(result).toBe(EXPECTED[browserName])
})
