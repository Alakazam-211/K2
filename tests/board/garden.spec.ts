// The Board Garden's own browser checks (Chromium and WebKit):
//   - a pasted picture: the original PNG bytes stored as a blob, the object holds its ref, and the board shows
//     the decoded picture; a dropped picture is placed where it was dropped;
//   - PNG export: drawn through SVG + canvas and encoded by K2Png, on a normal page AND inside a sealed frame
//     (sandbox="allow-scripts", an opaque origin, like K2's Garden): the picture must show the board, not a
//     blank or tainted canvas;
//   - a 1,000-object board: load time, and pan/zoom frame times (printed for both engines).
/* eslint-disable */
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { DIR, FAKE, K2PNG, openBoard } from './harness'

const ONE_BOARD = (objects: any[] = []) => ({
  seed: [
    ['seeded', { at: '2026-01-01T00:00:00Z' }],
    ['boards/b1', { id: 'b1', name: 'Test', project: null, format: 1 }],
    ['board/b1/meta', { format: 1, title: 'Test' }],
    ...objects.map((o) => ['board/b1/obj/' + o.id, o]),
  ],
})

// A real PNG made in the page (K2Png from canvas pixels): w×h, filled with one colour.
const makePng = (w: number, h: number, rgb: [number, number, number]) => `(async () => {
  const c = document.createElement('canvas'); c.width = ${w}; c.height = ${h}
  const g = c.getContext('2d'); g.fillStyle = 'rgb(${rgb.join(',')})'; g.fillRect(0, 0, ${w}, ${h})
  return K2Png.encode(c)
})()`

test('a pasted picture is stored as its original bytes and shown decoded; a dropped one lands where it fell', async ({ page }) => {
  await openBoard(page, ONE_BOARD(), { board: 'b1' })
  const r = await page.evaluate(async (make) => {
    const w = window as any
    const bytes: Uint8Array = await eval(make)
    const file = new File([bytes], 'Red square.png', { type: 'image/png' })
    const dt = new DataTransfer(); dt.items.add(file)
    const before = w.BoardApp.model.all().length
    document.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }))
    for (let i = 0; i < 200 && w.BoardApp.model.all().length === before; i++) await new Promise((r) => setTimeout(r, 10))
    const o = w.BoardApp.model.all().find((x: any) => x.type === 'image')
    if (!o) return { error: 'no image object' }
    const put = w.__fake.log.blobPuts[0]
    const stored = w.__fake.blobs.get(put.ref).bytes
    const same = stored.length === bytes.length && stored.every((b: number, i: number) => b === bytes[i])
    const img = document.querySelector('[data-id="' + o.id + '"] img') as HTMLImageElement
    for (let i = 0; i < 200 && !(img.complete && img.naturalWidth); i++) await new Promise((r) => setTimeout(r, 10))
    w.BoardApp.store.flushNow()
    for (let i = 0; i < 200 && !w.__fake.space.get('board/b1/obj/' + o.id); i++) await new Promise((r) => setTimeout(r, 10))
    return { src: o.src, ref: put.ref, same, w: o.w, h: o.h, alt: o.alt, natural: [img.naturalWidth, img.naturalHeight], storedSrc: (w.__fake.space.get('board/b1/obj/' + o.id) || {}).value?.src }
  }, makePng(48, 32, [220, 30, 30]))
  expect(r.error).toBeUndefined()
  expect(r.src).toMatch(/^k2blob:sha256-[0-9a-f]{64}$/)
  expect(r.src).toBe(r.ref)
  expect(r.storedSrc).toBe(r.ref)
  expect(r.same).toBe(true)
  expect([r.w, r.h, r.alt]).toEqual([48, 32, 'Red square'])
  expect(r.natural).toEqual([48, 32])

  // Dropped files (already stored by K2) are checked, then placed at the drop point.
  const d = await page.evaluate(async (make) => {
    const w = window as any
    const info = await w.__fake.putBytes(await eval(make), 'Blue.png')
    const before = w.BoardApp.model.all().filter((o: any) => o.type === 'image').length
    w.__fake.drop([{ ref: info.ref, name: 'Blue.png', mime: 'image/png', size: info.size }], 700, 450)
    for (let i = 0; i < 200 && w.BoardApp.model.all().filter((o: any) => o.type === 'image').length === before; i++) await new Promise((r) => setTimeout(r, 10))
    const o = w.BoardApp.model.all().find((x: any) => x.type === 'image' && x.src === info.ref)
    const at = w.BoardApp.toWorld(700, 450)
    return o ? { centre: [o.x + o.w / 2, o.y + o.h / 2], at } : null
  }, makePng(20, 20, [30, 30, 220]))
  expect(d).not.toBeNull()
  expect(Math.abs(d!.centre[0] - d!.at[0])).toBeLessThanOrEqual(1)
  expect(Math.abs(d!.centre[1] - d!.at[1])).toBeLessThanOrEqual(1)
  expect(await page.evaluate(() => (window as any).__errors)).toEqual([])
})

// What a PNG export shows: its size, the colour at a sticky's centre and at an image's centre, and how many
// distinct colours it has (a blank or failed drawing has one).
const PROBE = `(async () => {
  const w = window
  const bytes = await w.BoardApp.exportPng()
  const box = w.BoardApp.contentBox()
  const bm = await createImageBitmap(new Blob([bytes], { type: 'image/png' }))
  const c = document.createElement('canvas'); c.width = bm.width; c.height = bm.height
  const g = c.getContext('2d'); g.drawImage(bm, 0, 0)
  const px = (x, y) => Array.from(g.getImageData(Math.round(x - box.x + 40), Math.round(y - box.y + 40), 1, 1).data)
  const all = g.getImageData(0, 0, c.width, c.height).data; const seen = new Set()
  for (let i = 0; i < all.length; i += 4 * 97) seen.add(all[i] + ',' + all[i + 1] + ',' + all[i + 2])
  const s = w.BoardApp.model.get('s-red'); const i = w.BoardApp.model.get('i-blue')
  return { signature: Array.from(bytes.slice(0, 4)), size: [bm.width, bm.height], box: [box.w, box.h], sticky: px(s.x + 30, s.y + 30), image: px(i.x + i.w / 2, i.y + i.h / 2), colours: seen.size }
})()`
const PNG_OBJECTS = [
  { id: 's-red', type: 'sticky', x: 0, y: 0, w: 200, h: 200, z: 1, text: 'Export me', color: 'red' },
  { id: 'h-shape', type: 'shape', x: 260, y: 0, w: 180, h: 120, z: 2, shape: 'ellipse', fill: 'green', text: 'Shape' },
]
function checkPng(r: any) {
  expect(r.error).toBeUndefined()
  expect(r.signature).toEqual([0x89, 0x50, 0x4e, 0x47])
  expect(r.size[0]).toBe(Math.ceil(r.box[0] + 80)); expect(r.size[1]).toBe(Math.ceil(r.box[1] + 80))
  expect(r.colours).toBeGreaterThan(5)
  // The red sticky: light red (#ffb8b3), not the white background or a transparent blank.
  const [R, G, B, A] = r.sticky
  expect(A).toBe(255); expect(R).toBeGreaterThan(230); expect(G).toBeLessThan(215); expect(B).toBeLessThan(215)
  // The stored picture (a blob drawn as a data: URL): blue.
  const [r2, g2, b2] = r.image
  expect(b2).toBeGreaterThan(150); expect(r2).toBeLessThan(90); expect(g2).toBeLessThan(90)
}

test('PNG export draws the board (normal page)', async ({ page, browserName }) => {
  await openBoard(page, ONE_BOARD(PNG_OBJECTS), { board: 'b1', objects: 2 })
  await page.evaluate(async (make) => {
    const w = window as any
    const info = await w.__fake.putBytes(await eval(make), 'Blue.png')
    w.BoardApp.model.apply([{ op: 'add', obj: { id: 'i-blue', type: 'image', x: 0, y: 260, w: 120, h: 80, src: info.ref } }])
    const img = document.querySelector('[data-id="i-blue"] img') as HTMLImageElement
    for (let i = 0; i < 300 && !(img.complete && img.naturalWidth); i++) await new Promise((r) => setTimeout(r, 10))
  }, makePng(60, 40, [20, 40, 230]))
  const r = await page.evaluate(PROBE)
  console.log(`[${browserName}] PNG export (page): ${JSON.stringify(r)}`)
  checkPng(r)
  // Through the Export panel: K2's download gets a PNG.
  await page.click('[aria-label="Export"]')
  await page.click('#pop [aria-label="Download a picture of the board (PNG)"]')
  await page.waitForFunction(() => (window as any).__fake.log.downloads.length === 1)
  const dl = await page.evaluate(() => { const d = (window as any).__fake.log.downloads[0]; return { name: d.name, mime: d.mime, head: Array.from(d.data.slice(0, 4)), n: d.data.length } })
  expect(dl).toMatchObject({ name: 'Test.png', mime: 'image/png', head: [0x89, 0x50, 0x4e, 0x47] })
  expect(dl.n).toBeGreaterThan(1000)
})

test('PNG export draws the board inside a sealed frame (opaque origin, like the Garden)', async ({ page, browserName }) => {
  // One self-contained page, as K2's frame gets it: CSS, K2Png, the fake K2 and the scripts inlined.
  const html = readFileSync(resolve(DIR, 'index.html'), 'utf8')
  const order = [...html.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => m[1])
  const css = readFileSync(resolve(DIR, 'board.css'), 'utf8')
  const body = html.replace(/^[\s\S]*<body>/, '').replace(/<\/body>[\s\S]*$/, '').replace(/<script src="[^"]+"><\/script>\s*/g, '')
  const safe = (s: string) => s.replace(/<\/script/gi, '<\\/script')
  const opts = ONE_BOARD(PNG_OBJECTS)
  const srcdoc = '<!doctype html><html><head><meta charset="utf-8"><style>' + css + '</style>' +
    '<script>window.__errors = []; addEventListener("error", (e) => window.__errors.push(String(e.message)))</script>' +
    '<script>' + safe(K2PNG) + '</script><script>' + safe(FAKE) + '</script>' +
    '<script>window.__fake = makeFakeK2(' + JSON.stringify(opts) + '); window.k2 = window.__fake.k2</script>' +
    '</head><body>' + body + order.map((f) => '<script>' + safe(readFileSync(resolve(DIR, f), 'utf8')) + '</script>').join('') +
    '<script>(async () => { let out; try {' +
    '  const w = window; for (let i = 0; i < 500 && !(w.BoardApp && w.BoardApp.store.boardId === "b1" && w.BoardApp.model.all().length >= 2); i++) await new Promise((r) => setTimeout(r, 10));' +
    '  const c = document.createElement("canvas"); c.width = 60; c.height = 40; const g = c.getContext("2d"); g.fillStyle = "rgb(20,40,230)"; g.fillRect(0, 0, 60, 40);' +
    '  const info = await w.__fake.putBytes(await K2Png.encode(c), "Blue.png");' +
    '  w.BoardApp.model.apply([{ op: "add", obj: { id: "i-blue", type: "image", x: 0, y: 260, w: 120, h: 80, src: info.ref } }]);' +
    '  const img = document.querySelector(\'[data-id="i-blue"] img\'); for (let i = 0; i < 300 && !(img.complete && img.naturalWidth); i++) await new Promise((r) => setTimeout(r, 10));' +
    '  out = await ' + PROBE + ' } catch (e) { out = { error: String(e && (e.stack || e.message || e)) } }' +
    '  out.origin = String(self.origin); out.secure = self.isSecureContext; out.errors = window.__errors; parent.postMessage({ boardPng: out }, "*")' +
    '})()</script></body></html>'
  // The host page is on a secure origin (as K2's app is); the frame itself is sandboxed to an opaque origin.
  await page.setViewportSize({ width: 1400, height: 900 })
  await page.route('http://localhost:58123/**', (route) => route.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html><body style="margin:0"></body>' }))
  await page.goto('http://localhost:58123/host.html')
  const r: any = await page.evaluate((doc) => new Promise((resolve) => {
    addEventListener('message', (e: MessageEvent) => { if (e.data && e.data.boardPng) resolve(e.data.boardPng) })
    const f = document.createElement('iframe')
    f.setAttribute('sandbox', 'allow-scripts'); f.style.width = '1400px'; f.style.height = '900px'; f.style.border = '0'
    f.srcdoc = doc
    document.body.appendChild(f)
  }), srcdoc)
  console.log(`[${browserName}] PNG export (sealed frame): ${JSON.stringify(r)}`)
  expect(r.origin).toBe('null')
  checkPng(r)
  expect(r.errors).toEqual([])
})

test('a 1,000-object board loads and pans and zooms smoothly (numbers printed per engine)', async ({ page, browserName }) => {
  const objects: any[] = []
  const COLS = 25
  for (let i = 0; i < 1000; i++) {
    const x = (i % COLS) * 240; const y = Math.floor(i / COLS) * 240
    if (i % 10 === 9) objects.push({ id: 'c-' + String(i).padStart(10, '0'), type: 'connector', from: { id: 's-' + String(i - 1).padStart(10, '0') }, to: { id: 's-' + String(i - 2).padStart(10, '0') }, z: i })
    else if (i % 5 === 4) objects.push({ id: 's-' + String(i).padStart(10, '0'), type: 'shape', shape: 'round', x, y, w: 180, h: 120, z: i, text: 'Shape ' + i })
    else objects.push({ id: 's-' + String(i).padStart(10, '0'), type: 'sticky', x, y, w: 200, h: 200, z: i, text: 'Note ' + i + ' with a few words', color: ['yellow', 'pink', 'blue', 'green'][i % 4] })
  }
  const t0 = Date.now()
  await openBoard(page, ONE_BOARD(objects), { board: 'b1', objects: 1000 })
  const load = Date.now() - t0
  const dom = await page.evaluate(() => document.querySelectorAll('#world .obj').length)
  expect(dom).toBe(1000)
  // Pan and zoom: 60 camera changes, each awaited to the next painted frame.
  const frames = await page.evaluate(async () => {
    const w = window as any; const out: number[] = []
    const frame = () => new Promise<number>((r) => requestAnimationFrame((t) => r(t)))
    await frame()
    for (let i = 0; i < 60; i++) {
      const t = performance.now()
      if (i % 2) { w.BoardApp.cam.x -= 40; w.BoardApp.cam.y -= 25; w.BoardApp.applyCam() } else w.BoardApp.zoomAt(700, 450, w.BoardApp.cam.s * (i % 4 ? 1.08 : 0.93))
      await frame(); await frame()
      out.push(performance.now() - t)
    }
    out.sort((a, b) => a - b)
    return { median: out[30], p95: out[57], max: out[59] }
  })
  // Real wheel zoom through the page (Ctrl+wheel), timed end to end.
  const w0 = Date.now()
  await page.mouse.move(700, 450)
  await page.keyboard.down('Control')
  for (let i = 0; i < 10; i++) await page.mouse.wheel(0, i % 2 ? 120 : -120)
  await page.keyboard.up('Control')
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))))
  const wheel = Date.now() - w0
  const line = `[${browserName}] 1,000 objects: load ${load} ms (to the board drawn); pan/zoom frame median ${frames.median.toFixed(1)} ms, p95 ${frames.p95.toFixed(1)} ms, max ${frames.max.toFixed(1)} ms; 10 Ctrl+wheel zooms ${wheel} ms`
  console.log(line)
  test.info().annotations.push({ type: 'perf', description: line })
  expect(load).toBeLessThan(15000)
  expect(frames.p95).toBeLessThan(250)
  expect(await page.evaluate(() => (window as any).__errors)).toEqual([])
})
