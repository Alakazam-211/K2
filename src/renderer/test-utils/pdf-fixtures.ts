// Tiny PDFs for the PDF viewer's tests and shots, written by hand: no PDF
// library, so nothing new to licence. Each is built in memory from the
// objects below, with a correct xref, uncompressed so a test can read it.
//
//   drawingPdf(): a two-sheet "CAD export" (792 x 612 pt, landscape):
//     - four layers (optional content groups): A-WALL, E-LITE and M-HVAC on,
//       A-ANNO-DIMS off in the default config, each drawing one coloured
//       block (LAYER_BOXES says where), wrapped in /OC BDC … EMC;
//     - sheet 1 annotations: a comment (Text, /NM c-001, with a Popup), a
//       Square markup, a revision cloud (Polygon, /BE /S /C, with a cloud
//       appearance stream as CAD and markup tools write it), a FreeText, an
//       Approved Stamp, a comment with no author, a Link to sheet 2 and a
//       Link to https://example.com/spec;
//     - sheet 2: an Ink markup dated only by /CreationDate;
//     - text to find: "Door" three times on sheet 1 and once on sheet 2.
//   drawingPdf({ unusedLayers }): the same, plus layers that are only names:
//     in the config, but no content is marked with them (as many CAD
//     exports ship hundreds of);
//   namesOnlyPdf(n): one sheet whose n layers are all names only;
//   plainPdf(): one page of text, no layers and no annotations.
//
// `bun src/renderer/test-utils/write-pdf-fixtures.ts <dir>` writes both as
// files (the shots harness serves them through fs/read-binary).

export const PAGE_W = 792
export const PAGE_H = 612

/** Where each layer draws its block (PDF points, origin bottom left). */
export const LAYER_BOXES = {
  'A-WALL': { x: 100, y: 300, w: 300, h: 30, rgb: [0.1, 0.2, 0.8] },
  'E-LITE': { x: 450, y: 300, w: 60, h: 60, rgb: [0.9, 0.6, 0.1] },
  'A-ANNO-DIMS': { x: 100, y: 200, w: 300, h: 10, rgb: [0.1, 0.6, 0.2] },
  'M-HVAC': { x: 550, y: 150, w: 120, h: 40, rgb: [0.6, 0.1, 0.6] },
} as const
export type LayerName = keyof typeof LAYER_BOXES

/** The revision cloud's polygon (a square), and how far its scallops bulge. */
export const CLOUD = { x0: 440, y0: 380, x1: 520, y1: 460, bulge: 6 }

class PdfWriter {
  private objs: string[] = []
  /** Reserve an object number; fill it later with set(). */
  ref(): number {
    this.objs.push('')
    return this.objs.length
  }
  add(body: string): number {
    this.objs.push(body)
    return this.objs.length
  }
  set(n: number, body: string): void {
    this.objs[n - 1] = body
  }
  stream(dict: string, data: string): number {
    return this.add(`<< ${dict} /Length ${data.length} >>\nstream\n${data}\nendstream`)
  }
  build(root: number): Uint8Array {
    let out = '%PDF-1.7\n%\xe2\xe3\xcf\xd3\n'
    const offsets: number[] = []
    this.objs.forEach((body, i) => {
      offsets.push(out.length)
      out += `${i + 1} 0 obj\n${body}\nendobj\n`
    })
    const xref = out.length
    out += `xref\n0 ${this.objs.length + 1}\n0000000000 65535 f \n`
    for (const o of offsets) out += `${String(o).padStart(10, '0')} 00000 n \n`
    out += `trailer\n<< /Size ${this.objs.length + 1} /Root ${root} 0 R >>\nstartxref\n${xref}\n%%EOF\n`
    const bytes = new Uint8Array(out.length)
    for (let i = 0; i < out.length; i++) bytes[i] = out.charCodeAt(i) & 0xff
    return bytes
  }
}

const text = (size: number, x: number, y: number, s: string): string => `BT /F1 ${size} Tf ${x} ${y} Td (${s}) Tj ET`
const rgbOf = (c: readonly number[]): string => c.join(' ')

/** One scalloped edge from (ax, ay) to (bx, by), bulging to the left of travel (outward for a counter-clockwise polygon). */
function scallops(ax: number, ay: number, bx: number, by: number, n: number, bulge: number): string {
  const dx = (bx - ax) / n
  const dy = (by - ay) / n
  const len = Math.hypot(dx, dy)
  // Outward normal for a counter-clockwise walk: rotate the direction right.
  const nx = (dy / len) * bulge * 1.33
  const ny = (-dx / len) * bulge * 1.33
  const out: string[] = []
  for (let i = 0; i < n; i++) {
    const sx = ax + dx * i
    const sy = ay + dy * i
    const ex = sx + dx
    const ey = sy + dy
    out.push(`${(sx + nx).toFixed(2)} ${(sy + ny).toFixed(2)} ${(ex + nx).toFixed(2)} ${(ey + ny).toFixed(2)} ${ex.toFixed(2)} ${ey.toFixed(2)} c`)
  }
  return out.join('\n')
}

export function cloudAppearance(): string {
  const { x0, y0, x1, y1, bulge } = CLOUD
  return [
    '1 0 0 RG 1.5 w',
    `${x0} ${y0} m`,
    scallops(x0, y0, x1, y0, 5, bulge),
    scallops(x1, y0, x1, y1, 5, bulge),
    scallops(x1, y1, x0, y1, 5, bulge),
    scallops(x0, y1, x0, y0, 5, bulge),
    'S',
  ].join('\n')
}

export function drawingPdf(opts: { unusedLayers?: string[] } = {}): Uint8Array {
  const w = new PdfWriter()
  const catalog = w.ref()
  const pages = w.ref()
  const font = w.add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>')
  const names = Object.keys(LAYER_BOXES) as LayerName[]
  const ocg = Object.fromEntries(names.map((n) => [n, w.add(`<< /Type /OCG /Name (${n}) >>`)])) as Record<LayerName, number>
  const unused = (opts.unusedLayers ?? []).map((n) => w.add(`<< /Type /OCG /Name (${n}) >>`))
  const page1 = w.ref()
  const page2 = w.ref()

  const props = names.map((n, i) => `/oc${i + 1} ${ocg[n]} 0 R`).join(' ')
  const resources = `<< /Font << /F1 ${font} 0 R >> /Properties << ${props} >> >>`

  const layerOps = names
    .map((n, i) => {
      const b = LAYER_BOXES[n]
      return `/OC /oc${i + 1} BDC ${rgbOf(b.rgb)} rg ${b.x} ${b.y} ${b.w} ${b.h} re f EMC`
    })
    .join('\n')
  const c1 = w.stream(
    '',
    [
      '0 0 0 RG 2 w 20 20 752 572 re S',
      '0 0 0 rg',
      text(18, 40, 560, 'GROUND FLOOR PLAN'),
      text(10, 640, 40, 'Sheet A-102'),
      text(10, 40, 40, 'Specification'),
      layerOps,
      '0 0 0 rg',
      text(11, 100, 420, 'Door D1'),
      text(11, 300, 420, 'Door D2'),
      text(11, 160, 250, 'Door D4'),
      text(11, 600, 420, 'Window W1'),
    ].join('\n'),
  )
  const c2 = w.stream('', ['0 0 0 RG 2 w 20 20 752 572 re S', '0 0 0 rg', text(18, 40, 560, 'SECOND FLOOR PLAN'), text(11, 100, 420, 'Door D3')].join('\n'))

  const form = (bbox: number[], ops: string): number => w.stream(`/Type /XObject /Subtype /Form /BBox [${bbox.join(' ')}] /Resources << /Font << /F1 ${font} 0 R >> >>`, ops)

  // Sheet 1 annotations, in /Annots order.
  const comment = w.ref()
  const popup = w.add(`<< /Type /Annot /Subtype /Popup /Parent ${comment} 0 R /Rect [620 420 760 500] /Open false >>`)
  w.set(
    comment,
    `<< /Type /Annot /Subtype /Text /Rect [600 500 620 520] /Name /Comment /C [1 0.8 0] /T (Sam Reyes) /Contents (Check the door swing at D1.) /M (D:20261112093000Z) /CreationDate (D:20261112090000Z) /NM (c-001) /Popup ${popup} 0 R /AP << /N ${form([600, 500, 620, 520], '1 0.8 0 rg 0 0 0 RG 600 500 20 20 re B')} 0 R >> >>`,
  )
  const square = w.add('<< /Type /Annot /Subtype /Square /Rect [90 290 410 340] /C [1 0 0] /BS << /W 2 >> /T (Brook Lane) /Contents (Wall thickness to be confirmed.) /M (D:20261110141500+01\'00\') >>')
  const cloud = w.add(
    `<< /Type /Annot /Subtype /Polygon /Rect [${CLOUD.x0 - 12} ${CLOUD.y0 - 12} ${CLOUD.x1 + 12} ${CLOUD.y1 + 12}] /Vertices [${CLOUD.x0} ${CLOUD.y0} ${CLOUD.x1} ${CLOUD.y0} ${CLOUD.x1} ${CLOUD.y1} ${CLOUD.x0} ${CLOUD.y1}] /BE << /S /C /I 1 >> /C [1 0 0] /IT /PolygonCloud /T (Sam Reyes) /Contents (Revised light fitting position.) /NM (rev-cloud-2) /M (D:20261113080000Z) /AP << /N ${form([CLOUD.x0 - 12, CLOUD.y0 - 12, CLOUD.x1 + 12, CLOUD.y1 + 12], cloudAppearance())} 0 R >> >>`,
  )
  const freeText = w.add(
    `<< /Type /Annot /Subtype /FreeText /Rect [100 450 300 475] /DA (/Helv 10 Tf 0 0 1 rg) /T (Brook Lane) /Contents (Verify on site) /M (D:20261111120000Z) /AP << /N ${form([100, 450, 300, 475], '0 0 1 RG 1 w 100 450 200 25 re S 0 0 1 rg ' + text(10, 106, 458, 'Verify on site'))} 0 R >> >>`,
  )
  const stamp = w.add(
    `<< /Type /Annot /Subtype /Stamp /Name /Approved /Rect [600 60 740 110] /T (QA Desk) /Contents (Approved for construction) /M (D:20261114100000Z) /AP << /N ${form([600, 60, 740, 110], '0.8 0 0 RG 3 w 602 62 136 46 re S 0.8 0 0 rg ' + text(18, 616, 78, 'APPROVED'))} 0 R >> >>`,
  )
  const anonymous = w.add('<< /Type /Annot /Subtype /Text /Rect [700 500 720 520] /Name /Note /Contents (Who left this?) >>')
  const linkPage = w.add(`<< /Type /Annot /Subtype /Link /Rect [636 36 720 52] /Border [0 0 0] /Dest [${page2} 0 R /Fit] >>`)
  const linkUrl = w.add('<< /Type /Annot /Subtype /Link /Rect [36 36 120 52] /Border [0 0 0] /A << /S /URI /URI (https://example.com/spec) >> >>')

  // Sheet 2.
  const ink = w.add('<< /Type /Annot /Subtype /Ink /Rect [190 290 410 360] /InkList [[200 300 260 350 320 300 400 350]] /C [0 0 1] /BS << /W 2 >> /T (Sam Reyes) /Contents (Mark-up from the site visit.) /CreationDate (D:20261111160000Z) >>')

  const annots1 = [comment, popup, square, cloud, freeText, stamp, anonymous, linkPage, linkUrl].map((n) => `${n} 0 R`).join(' ')
  w.set(page1, `<< /Type /Page /Parent ${pages} 0 R /MediaBox [0 0 ${PAGE_W} ${PAGE_H}] /Resources ${resources} /Contents ${c1} 0 R /Annots [${annots1}] >>`)
  w.set(page2, `<< /Type /Page /Parent ${pages} 0 R /MediaBox [0 0 ${PAGE_W} ${PAGE_H}] /Resources ${resources} /Contents ${c2} 0 R /Annots [${ink} 0 R] >>`)
  w.set(pages, `<< /Type /Pages /Kids [${page1} 0 R ${page2} 0 R] /Count 2 >>`)
  const all = [...names.map((n) => ocg[n]), ...unused].map((n) => `${n} 0 R`).join(' ')
  w.set(catalog, `<< /Type /Catalog /Pages ${pages} 0 R /OCProperties << /OCGs [${all}] /D << /Name (Default) /BaseState /ON /Order [${all}] /OFF [${ocg['A-ANNO-DIMS']} 0 R] >> >> >>`)
  return w.build(catalog)
}

export function namesOnlyPdf(n: number): Uint8Array {
  const w = new PdfWriter()
  const catalog = w.ref()
  const pages = w.ref()
  const font = w.add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>')
  const ocgs = Array.from({ length: n }, (_, i) => w.add(`<< /Type /OCG /Name (L-${String(i + 1).padStart(3, '0')}) >>`))
  const page = w.ref()
  const c = w.stream('', ['0 0 0 RG 2 w 20 20 752 572 re S', '0 0 0 rg', text(18, 40, 560, 'SITE PLAN')].join('\n'))
  w.set(page, `<< /Type /Page /Parent ${pages} 0 R /MediaBox [0 0 ${PAGE_W} ${PAGE_H}] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${c} 0 R >>`)
  w.set(pages, `<< /Type /Pages /Kids [${page} 0 R] /Count 1 >>`)
  const all = ocgs.map((o) => `${o} 0 R`).join(' ')
  w.set(catalog, `<< /Type /Catalog /Pages ${pages} 0 R /OCProperties << /OCGs [${all}] /D << /Name (Default) /BaseState /ON /Order [${all}] >> >> >>`)
  return w.build(catalog)
}

export function plainPdf(): Uint8Array {
  const w = new PdfWriter()
  const catalog = w.ref()
  const pages = w.ref()
  const font = w.add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>')
  const page = w.ref()
  const c = w.stream('', ['0 0 0 rg', text(14, 72, 700, 'Site notes'), text(11, 72, 680, 'No layers and no annotations in this one.')].join('\n'))
  w.set(page, `<< /Type /Page /Parent ${pages} 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${c} 0 R >>`)
  w.set(pages, `<< /Type /Pages /Kids [${page} 0 R] /Count 1 >>`)
  w.set(catalog, `<< /Type /Catalog /Pages ${pages} 0 R >>`)
  return w.build(catalog)
}

/** The bytes as the daemon's fs/read-binary sends them. */
export function toBase64(bytes: Uint8Array): string {
  let s = ''
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i])
  return btoa(s)
}
