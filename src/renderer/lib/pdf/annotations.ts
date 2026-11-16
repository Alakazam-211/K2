// A PDF's annotations as the viewer lists them (P1): comments, markups and
// stamps, page by page, each with its author and date. pdf.js reads them
// (page.getAnnotations); raw-annots adds /NM and the cloudy border.
//   - key: /NM when the file has one, else "p<page>-<n>" (n counts the
//     page's annotations from 1, in pdf.js's order). P3's multiplayer markup
//     adds K2's own attribution beside /T, without reshaping the list.
//   - author: /T, else "Unknown author". date: /M, else /CreationDate.
//   - links, popups (a comment's note window) and form fields aren't listed:
//     links work on the page, popups are the comment itself.

import type { RawAnnotDict } from './raw-annots'

export const UNKNOWN_AUTHOR = 'Unknown author'

/** The fields of pdf.js's annotation data the viewer reads. */
export interface PdfjsAnnotData {
  id: string
  subtype: string
  rect: number[]
  titleObj?: { str: string }
  contentsObj?: { str: string }
  modificationDate?: string | null
  creationDate?: string | null
  hasAppearance?: boolean
}

export interface PdfAnnot {
  key: string
  /** pdf.js's id ("12R"): the annotation layer's data-annotation-id. */
  id: string
  page: number
  subtype: string
  /** What it is, in words ("Revision cloud", "Comment"). */
  label: string
  author: string
  date: Date | null
  text: string
  /** PDF points: [x1, y1, x2, y2]. */
  rect: number[]
  cloud: boolean
  hasAppearance: boolean
}

const LABEL: Record<string, string> = {
  Text: 'Comment',
  FreeText: 'Text box',
  Square: 'Rectangle',
  Circle: 'Ellipse',
  Polygon: 'Polygon',
  PolyLine: 'Polyline',
  Line: 'Line',
  Ink: 'Ink',
  Stamp: 'Stamp',
  Highlight: 'Highlight',
  Underline: 'Underline',
  StrikeOut: 'Strikeout',
  Squiggly: 'Squiggly underline',
  Caret: 'Caret',
  FileAttachment: 'Attachment',
  Sound: 'Sound',
}

export const isListed = (subtype: string): boolean => subtype in LABEL

export function labelOf(subtype: string, cloud: boolean): string {
  if (cloud && subtype === 'Polygon') return 'Revision cloud'
  if (cloud && subtype === 'PolyLine') return 'Cloud line'
  return LABEL[subtype] ?? subtype
}

/** A PDF date ("D:YYYYMMDDHHmmSSOHH'mm'"), or null when it isn't one. */
export function parsePdfDate(s: string | null | undefined): Date | null {
  if (!s) return null
  const m = /^(?:D:)?(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?\s*(?:([Zz+-])\s*(\d{2})?'?\s*(\d{2})?'?)?/.exec(s.trim())
  if (!m) return null
  const n = (v: string | undefined, d: number): number => (v === undefined ? d : Number(v))
  const [y, mo, d, h, mi, se] = [n(m[1], 0), n(m[2], 1), n(m[3], 1), n(m[4], 0), n(m[5], 0), n(m[6], 0)]
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59 || se > 59) return null
  let t = Date.UTC(y, mo - 1, d, h, mi, se)
  if (m[7] === '+' || m[7] === '-') {
    const off = (n(m[8], 0) * 60 + n(m[9], 0)) * 60_000
    t += m[7] === '+' ? -off : off
  }
  return new Date(t)
}

export function annotKey(nm: string | undefined, page: number, n: number): string {
  return nm && nm.trim() ? nm : `p${page}-${n}`
}

/** One page's annotations (pdf.js order), as the list's rows. */
export function pageAnnotations(page: number, items: readonly PdfjsAnnotData[], raw: ReadonlyMap<string, RawAnnotDict>): PdfAnnot[] {
  const out: PdfAnnot[] = []
  items.forEach((a, i) => {
    if (!isListed(a.subtype)) return
    const r = raw.get(a.id)
    const cloud = !!r?.cloud && (a.subtype === 'Polygon' || a.subtype === 'PolyLine')
    out.push({
      key: annotKey(r?.nm, page, i + 1),
      id: a.id,
      page,
      subtype: a.subtype,
      label: labelOf(a.subtype, cloud),
      author: a.titleObj?.str.trim() || UNKNOWN_AUTHOR,
      date: parsePdfDate(a.modificationDate) ?? parsePdfDate(a.creationDate),
      text: a.contentsObj?.str ?? '',
      rect: a.rect,
      cloud,
      hasAppearance: !!a.hasAppearance,
    })
  })
  return out
}

interface DocLike {
  numPages: number
  getPage(n: number): Promise<{ getAnnotations(o?: { intent?: string }): Promise<unknown[]> }>
}

/** Every page's listed annotations, in page order. */
export async function readAnnotations(doc: DocLike, raw: ReadonlyMap<string, RawAnnotDict>): Promise<PdfAnnot[]> {
  const out: PdfAnnot[] = []
  for (let n = 1; n <= doc.numPages; n++) {
    const page = await doc.getPage(n)
    out.push(...pageAnnotations(n, (await page.getAnnotations({ intent: 'display' })) as PdfjsAnnotData[], raw))
  }
  return out
}

export function groupByPage(list: readonly PdfAnnot[]): { page: number; items: PdfAnnot[] }[] {
  const out: { page: number; items: PdfAnnot[] }[] = []
  for (const a of list) {
    const last = out[out.length - 1]
    if (last && last.page === a.page) last.items.push(a)
    else out.push({ page: a.page, items: [a] })
  }
  return out
}

export interface SubtypeFilter {
  /** "all", or the label it keeps. */
  id: string
  label: string
  count: number
}

/** All, then one filter per kind present, in the order they first appear. */
export function subtypeFilters(list: readonly PdfAnnot[]): SubtypeFilter[] {
  const counts = new Map<string, number>()
  for (const a of list) counts.set(a.label, (counts.get(a.label) ?? 0) + 1)
  return [{ id: 'all', label: 'All', count: list.length }, ...[...counts].map(([label, count]) => ({ id: label, label, count }))]
}

export const applyFilter = (list: readonly PdfAnnot[], filter: string): PdfAnnot[] => (filter === 'all' ? [...list] : list.filter((a) => a.label === filter))
