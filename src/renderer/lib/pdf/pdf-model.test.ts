// The PDF viewer's model (P1), on real pdf.js (the legacy build, which runs
// in Node and draws through @napi-rs/canvas) and the hand-made fixtures in
// test-utils/pdf-fixtures.ts:
//   - layers come from the PDF's default config, and toggling one changes
//     what's drawn (pixels in the layer's block);
//   - a revision cloud draws its scallops through pdf.js;
//   - annotations: author from /T (else "Unknown author"), date from /M or
//     /CreationDate, keyed by /NM when present, else page and index; links
//     and popups aren't listed;
//   - find counts matches across pages, case-blind by default.

import { describe, expect, it } from 'vitest'
import * as pdfjs from 'pdfjs-dist/legacy/build/pdf.mjs'
import { CLOUD, LAYER_BOXES, PAGE_H, drawingPdf, namesOnlyPdf, plainPdf, type LayerName } from '@/test-utils/pdf-fixtures'
import { countCoverage, coverageSummary, layerView, type Coverage } from './layer-coverage'
import { layerSummary, readLayers, setAllLayers, setLayer, visibleCount } from './layers'
import { UNKNOWN_AUTHOR, groupByPage, parsePdfDate, readAnnotations, subtypeFilters } from './annotations'
import { scanAnnotDicts } from './raw-annots'
import { findMatches, matchSegments, pageText } from './find'

// pdf.js's own copies of the 14 standard fonts (a browser uses the system's).
const FONTS = `${process.cwd()}/node_modules/pdfjs-dist/standard_fonts/`

const binaryOf = (b: Uint8Array): string => Array.from(b, (c) => String.fromCharCode(c)).join('')

async function open(bytes: Uint8Array) {
  // pdf.js takes the buffer over: hand it a copy.
  return pdfjs.getDocument({ data: bytes.slice(), isEvalSupported: false, standardFontDataUrl: FONTS, verbosity: 0 }).promise
}

type Doc = Awaited<ReturnType<typeof open>>

/** Draw sheet 1 at scale 1 with this config and read one pixel (PDF points). */
async function drawn(doc: Doc, cfg: unknown): Promise<(x: number, y: number) => number[]> {
  const page = await doc.getPage(1)
  const viewport = page.getViewport({ scale: 1 })
  const { canvas, context } = (doc as unknown as { canvasFactory: { create: (w: number, h: number) => { canvas: unknown; context: CanvasRenderingContext2D } } }).canvasFactory.create(viewport.width, viewport.height)
  await page.render({ canvas, viewport, optionalContentConfigPromise: Promise.resolve(cfg) } as never).promise
  return (x, y) => Array.from(context.getImageData(x, PAGE_H - y, 1, 1).data)
}
const centre = (name: LayerName): [number, number] => {
  const b = LAYER_BOXES[name]
  return [b.x + b.w / 2, b.y + b.h / 2]
}
const WHITE = [255, 255, 255, 255]

describe('layers', () => {
  it('reads every layer, in order, with the default config’s visibility', async () => {
    const doc = await open(drawingPdf())
    const layers = readLayers(await doc.getOptionalContentConfig())
    expect(layers.map((l) => [l.name, l.visible])).toEqual([
      ['A-WALL', true],
      ['E-LITE', true],
      ['A-ANNO-DIMS', false],
      ['M-HVAC', true],
    ])
    expect(visibleCount(layers)).toBe(3)
    expect(layerSummary(layers)).toBe('3 of 4 layers')
  })

  it('has none for a PDF without optional content', async () => {
    const doc = await open(plainPdf())
    expect(readLayers(await doc.getOptionalContentConfig())).toEqual([])
    expect(layerSummary([])).toBe('No layers')
  })

  it('toggling a layer changes what is drawn', async () => {
    const doc = await open(drawingPdf())
    const cfg = await doc.getOptionalContentConfig()
    const id = (name: LayerName): string => readLayers(cfg).find((l) => l.name === name)!.id

    let px = await drawn(doc, cfg)
    expect(px(...centre('A-WALL'))).not.toEqual(WHITE)
    expect(px(...centre('A-ANNO-DIMS'))).toEqual(WHITE)

    setLayer(cfg, id('A-WALL'), false)
    setLayer(cfg, id('A-ANNO-DIMS'), true)
    expect(layerSummary(readLayers(cfg))).toBe('3 of 4 layers')
    px = await drawn(doc, cfg)
    expect(px(...centre('A-WALL'))).toEqual(WHITE)
    expect(px(...centre('A-ANNO-DIMS'))).toEqual([26, 153, 51, 255])

    setAllLayers(cfg, false)
    px = await drawn(doc, cfg)
    for (const name of Object.keys(LAYER_BOXES) as LayerName[]) expect(px(...centre(name)), name).toEqual(WHITE)
    expect(layerSummary(readLayers(cfg))).toBe('0 of 4 layers')

    setAllLayers(cfg, true)
    px = await drawn(doc, cfg)
    for (const name of Object.keys(LAYER_BOXES) as LayerName[]) expect(px(...centre(name)), name).not.toEqual(WHITE)
  })
})

describe('layer coverage', () => {
  /** Count every page's drawing per layer, as the tab does in the background. */
  async function coverage(bytes: Uint8Array) {
    const doc = await open(bytes)
    const layers = readLayers(await doc.getOptionalContentConfig())
    const counts = new Map<string, number>()
    for (let n = 1; n <= doc.numPages; n++) countCoverage(await (await doc.getPage(n)).getOperatorList(), pdfjs.OPS, counts)
    const cov: Coverage = { scanned: doc.numPages, total: doc.numPages, counts }
    return { layers, cov, byName: Object.fromEntries(layers.map((l) => [l.name, counts.get(l.id) ?? 0])) }
  }

  it('counts the drawing each layer covers', async () => {
    const { byName } = await coverage(drawingPdf())
    expect(byName).toEqual({ 'A-WALL': 1, 'E-LITE': 1, 'A-ANNO-DIMS': 1, 'M-HVAC': 1 })
  })

  it('sets apart layers that are only names, once every page is counted', async () => {
    const { layers, cov } = await coverage(drawingPdf({ unusedLayers: ['X-OLD-1', 'X-OLD-2'] }))
    const done = layerView(layers, cov)
    expect(done.checking).toBe(false)
    expect(done.used.map((l) => [l.name, l.items])).toEqual([
      ['A-WALL', 1],
      ['E-LITE', 1],
      ['A-ANNO-DIMS', 1],
      ['M-HVAC', 1],
    ])
    expect(done.empty.map((l) => l.name)).toEqual(['X-OLD-1', 'X-OLD-2'])
    expect(coverageSummary(done)).toBe('3 of 4 layers')
    // Half way through, nothing is empty yet.
    const half = layerView(layers, { ...cov, scanned: 1, total: 2 })
    expect(half.checking).toBe(true)
    expect(half.used).toHaveLength(6)
    expect(half.empty).toEqual([])
  })

  it('finds every layer empty in a file whose layers bind nothing', async () => {
    const { layers, cov } = await coverage(namesOnlyPdf(5))
    const v = layerView(layers, cov)
    expect(v.used).toEqual([])
    expect(v.empty).toHaveLength(5)
    expect(coverageSummary(v)).toBe('No layers in use')
  })
})

describe('revision clouds', () => {
  it('draw their scallops, outside the polygon’s straight edge', async () => {
    const doc = await open(drawingPdf())
    const px = await drawn(doc, await doc.getOptionalContentConfig())
    // A scallop's crest, half way along a bump on the bottom edge, below the straight edge.
    const bumpMid = CLOUD.x0 + (CLOUD.x1 - CLOUD.x0) / 10
    const crest = px(bumpMid, CLOUD.y0 - CLOUD.bulge)
    expect(crest[0]).toBeGreaterThan(200)
    expect(crest[1]).toBeLessThan(120)
    // Inside the cloud stays paper.
    expect(px((CLOUD.x0 + CLOUD.x1) / 2, (CLOUD.y0 + CLOUD.y1) / 2)).toEqual(WHITE)
  })
})

describe('annotations', () => {
  async function all(bytes = drawingPdf()) {
    const doc = await open(bytes)
    return readAnnotations(doc, scanAnnotDicts(binaryOf(bytes)))
  }

  it('lists comments, markups and stamps by page, not links or popups', async () => {
    const list = await all()
    expect(list.map((a) => [a.page, a.subtype])).toEqual([
      [1, 'Text'],
      [1, 'Square'],
      [1, 'Polygon'],
      [1, 'FreeText'],
      [1, 'Stamp'],
      [1, 'Text'],
      [2, 'Ink'],
    ])
    expect(groupByPage(list).map((g) => [g.page, g.items.length])).toEqual([
      [1, 6],
      [2, 1],
    ])
  })

  it('keys by /NM when present, else by page and index', async () => {
    const list = await all()
    expect(list.map((a) => a.key)).toEqual(['c-001', 'p1-2', 'rev-cloud-2', 'p1-4', 'p1-5', 'p1-6', 'p2-1'])
  })

  it('reads the author from /T and the date from /M, else /CreationDate', async () => {
    const list = await all()
    const by = (key: string) => list.find((a) => a.key === key)!
    expect(by('c-001')).toMatchObject({ author: 'Sam Reyes', text: 'Check the door swing at D1.', label: 'Comment' })
    expect(by('c-001').date?.toISOString()).toBe('2026-11-12T09:30:00.000Z')
    expect(by('p1-2').date?.toISOString()).toBe('2026-11-10T13:15:00.000Z')
    expect(by('p1-6').author).toBe(UNKNOWN_AUTHOR)
    expect(by('p1-6').date).toBeNull()
    expect(by('p2-1')).toMatchObject({ author: 'Sam Reyes', label: 'Ink' })
    expect(by('p2-1').date?.toISOString()).toBe('2026-11-11T16:00:00.000Z')
  })

  it('names a Polygon with a cloudy border a revision cloud', async () => {
    const list = await all()
    expect(list.find((a) => a.key === 'rev-cloud-2')).toMatchObject({ cloud: true, label: 'Revision cloud' })
    expect(list.find((a) => a.subtype === 'Square')).toMatchObject({ cloud: false, label: 'Rectangle' })
  })

  it('offers a filter per subtype present, with counts', async () => {
    expect(subtypeFilters(await all())).toEqual([
      { id: 'all', label: 'All', count: 7 },
      { id: 'Comment', label: 'Comment', count: 2 },
      { id: 'Rectangle', label: 'Rectangle', count: 1 },
      { id: 'Revision cloud', label: 'Revision cloud', count: 1 },
      { id: 'Text box', label: 'Text box', count: 1 },
      { id: 'Stamp', label: 'Stamp', count: 1 },
      { id: 'Ink', label: 'Ink', count: 1 },
    ])
  })

  it('has none in a plain PDF', async () => {
    expect(await all(plainPdf())).toEqual([])
  })

  it('parses PDF dates with and without offsets', () => {
    expect(parsePdfDate('D:20261112093000Z')?.toISOString()).toBe('2026-11-12T09:30:00.000Z')
    expect(parsePdfDate("D:20261110141500+01'00'")?.toISOString()).toBe('2026-11-10T13:15:00.000Z')
    expect(parsePdfDate("D:20261110141500-05'30")?.toISOString()).toBe('2026-11-10T19:45:00.000Z')
    expect(parsePdfDate('D:2026')?.toISOString()).toBe('2026-01-01T00:00:00.000Z')
    expect(parsePdfDate('yesterday')).toBeNull()
    expect(parsePdfDate(null)).toBeNull()
  })
})

describe('raw annotation dicts', () => {
  it('finds /NM and the cloudy border in uncompressed objects', () => {
    const dicts = scanAnnotDicts(binaryOf(drawingPdf()))
    const nm = [...dicts.values()].filter((d) => d.nm).map((d) => d.nm)
    expect(nm.sort()).toEqual(['c-001', 'rev-cloud-2'])
    expect([...dicts.values()].filter((d) => d.cloud)).toHaveLength(1)
  })

  it('unescapes PDF string escapes in /NM', () => {
    const d = scanAnnotDicts('7 0 obj\n<< /Type /Annot /NM (a\\(b\\)c\\\\d\\101) >>\nendobj\n')
    expect(d.get('7R')?.nm).toBe('a(b)c\\dA')
    const hex = scanAnnotDicts('8 2 obj << /NM <4142> >> endobj')
    expect(hex.get('8R2')?.nm).toBe('AB')
  })
})

describe('find', () => {
  async function texts() {
    const doc = await open(drawingPdf())
    const out = []
    for (let n = 1; n <= doc.numPages; n++) out.push(pageText((await (await doc.getPage(n)).getTextContent()).items as never))
    return out
  }

  it('counts every match across the pages, case-blind', async () => {
    const pages = await texts()
    const m = findMatches(pages, 'door')
    expect(m).toHaveLength(4)
    expect(m.map((x) => x.page)).toEqual([1, 1, 1, 2])
    expect(findMatches(pages, 'door', { matchCase: true })).toHaveLength(0)
    expect(findMatches(pages, 'Door D', { matchCase: true })).toHaveLength(4)
    expect(findMatches(pages, '')).toHaveLength(0)
    expect(findMatches(pages, 'deadline')).toHaveLength(0)
  })

  it('maps a match back to the text items it covers', () => {
    const page = pageText([{ str: 'Door ' }, { str: 'D1', hasEOL: true }, { str: 'next' }] as never)
    expect(page.text).toBe('Door D1 next')
    const [m] = findMatches([page], 'or d1 ne')
    expect(matchSegments(page, m)).toEqual([
      { item: 0, from: 2, to: 5 },
      { item: 1, from: 0, to: 2 },
      { item: 2, from: 0, to: 2 },
    ])
  })
})
