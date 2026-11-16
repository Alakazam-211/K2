// Each page's text, read once per document: the text layer draws from it
// and Find searches it, so a match's place in the text is a span in the
// layer (one span per item, the same order).

import { pageText, type PageText, type TextItemLike } from './find'

interface PageLike {
  getTextContent(): Promise<{ items: unknown[] }>
}
interface DocLike {
  numPages: number
  getPage(n: number): Promise<PageLike>
}

export interface CachedText {
  /** pdf.js's TextContent, for the text layer. */
  content: { items: unknown[] }
  text: PageText
}

const cache = new WeakMap<DocLike, Map<number, Promise<CachedText>>>()

export function pageTextOf(doc: DocLike, n: number): Promise<CachedText> {
  let pages = cache.get(doc)
  if (!pages) cache.set(doc, (pages = new Map()))
  let p = pages.get(n)
  if (!p) {
    p = doc
      .getPage(n)
      .then((page) => page.getTextContent())
      .then((content) => ({ content, text: pageText(content.items as TextItemLike[]) }))
    pages.set(n, p)
  }
  return p
}

/** Every page's text, page 1 first. */
export async function allPageText(doc: DocLike): Promise<PageText[]> {
  const out: PageText[] = []
  for (let n = 1; n <= doc.numPages; n++) out.push((await pageTextOf(doc, n)).text)
  return out
}
