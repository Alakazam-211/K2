// The link service pdf.js's annotation layer asks to bind links (its
// IPDFLinkService, the parts a single-page viewer needs):
//   - a link inside the PDF ("go to sheet 2", a named destination) gets a
//     "#…" href, which the app's link handler leaves alone, and a click
//     jumps to its page;
//   - a link out (http, https, mailto) is a plain <a href target=_blank>;
//     the app's capture-phase link handler (lib/external-link-handler.ts)
//     opens it in the default browser, as it does every other link;
//   - a "set layers" action (SetOCGState) changes this tab's layers;
//   - named actions page through (Next, Prev, First, Last).

export interface PdfDocForLinks {
  numPages: number
  getDestination(id: string): Promise<unknown[] | null>
  getPageIndex(ref: { num: number; gen: number }): Promise<number>
}

/** The 1-based page a destination points at, or null. */
export async function destinationPage(doc: PdfDocForLinks, dest: unknown): Promise<number | null> {
  const explicit = typeof dest === 'string' ? await doc.getDestination(dest) : dest
  if (!Array.isArray(explicit) || explicit.length === 0) return null
  const ref = explicit[0]
  let index: number | null = null
  if (ref && typeof ref === 'object' && 'num' in ref) index = await doc.getPageIndex(ref as { num: number; gen: number })
  else if (Number.isInteger(ref)) index = ref as number
  if (index === null || index < 0 || index >= doc.numPages) return null
  return index + 1
}

export interface PdfLinkTarget {
  doc: PdfDocForLinks
  page: () => number
  goToPage: (n: number) => void
  setOCGState: (action: { state: string[]; preserveRB: boolean }) => void
}

export function makeLinkService(t: PdfLinkTarget) {
  return {
    externalLinkEnabled: true,
    eventBus: null,
    isInPresentationMode: false,
    get pagesCount(): number {
      return t.doc.numPages
    },
    get page(): number {
      return t.page()
    },
    set page(n: number) {
      t.goToPage(n)
    },
    rotation: 0,
    addLinkAttributes(link: HTMLAnchorElement, url: string): void {
      link.href = url
      link.title = url
      link.target = '_blank'
      link.rel = 'noopener noreferrer nofollow'
    },
    getDestinationHash(dest: unknown): string {
      return `#${encodeURIComponent(typeof dest === 'string' ? dest : JSON.stringify(dest))}`
    },
    getAnchorUrl(anchor: string): string {
      return `#${anchor}`
    },
    async goToDestination(dest: unknown): Promise<void> {
      const n = await destinationPage(t.doc, dest)
      if (n !== null) t.goToPage(n)
    },
    goToPage(n: number): void {
      if (n >= 1 && n <= t.doc.numPages) t.goToPage(n)
    },
    executeNamedAction(action: string): void {
      const n = { NextPage: t.page() + 1, PrevPage: t.page() - 1, FirstPage: 1, LastPage: t.doc.numPages }[action]
      if (n !== undefined && n >= 1 && n <= t.doc.numPages) t.goToPage(n)
    },
    async executeSetOCGState(action: { state: string[]; preserveRB: boolean }): Promise<void> {
      t.setOCGState(action)
    },
    setHash(): void {},
    navigateTo(): void {},
  }
}

export type PdfLinkService = ReturnType<typeof makeLinkService>
