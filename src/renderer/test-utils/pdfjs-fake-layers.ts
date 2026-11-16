// pdf.js for the PDF viewer's component tests: the real library (the legacy
// build, which runs under Node) reads the document, its layers, annotations
// and text, and only drawing is faked, since jsdom has no canvas:
//   - page.render records what it was asked to draw (the page, the canvas,
//     and each layer's visibility from optionalContentConfigPromise);
//   - TextLayer writes one span per text item, as pdf.js does;
//   - AnnotationLayer writes one section per annotation, and binds links
//     through the link service the way pdf.js's LinkAnnotationElement does.
// Use: vi.mock('pdfjs-dist', () => import('@/test-utils/pdfjs-fake-layers')).

import * as real from 'pdfjs-dist/legacy/build/pdf.mjs'

export interface RenderCall {
  page: number
  canvas: HTMLCanvasElement | null
  /** Layer name → visible, as the config said at draw time. */
  layers: Record<string, boolean>
}

export const renders: RenderCall[] = []
export const resetRenders = (): void => void renders.splice(0)

export const OPS = real.OPS

/** Hold every page's operator list (the layer coverage count) until the returned call. */
let gate: Promise<void> | null = null
export function holdOperatorLists(): () => void {
  let release = (): void => {}
  gate = new Promise((r) => (release = r))
  return () => {
    gate = null
    release()
  }
}

/** Ours, not the real one: the viewer sets a browser worker path that Node can't load. */
export const GlobalWorkerOptions = { workerSrc: '' }

interface PageLike {
  render: (p: { canvas?: HTMLCanvasElement; optionalContentConfigPromise?: Promise<unknown> }) => unknown
}

export function getDocument(src: Parameters<typeof real.getDocument>[0]) {
  const task = real.getDocument({ ...(src as object), isEvalSupported: false, verbosity: 0, standardFontDataUrl: `${process.cwd()}/node_modules/pdfjs-dist/standard_fonts/` })
  const promise = task.promise.then((doc) => {
    const getPage = doc.getPage.bind(doc)
    doc.getPage = async (n: number) => {
      const page = await getPage(n)
      const getOperatorList = page.getOperatorList.bind(page)
      page.getOperatorList = (async (...a: Parameters<typeof getOperatorList>) => {
        if (gate) await gate
        return getOperatorList(...a)
      }) as typeof getOperatorList
      ;(page as unknown as PageLike).render = (p) => {
        let cancelled = false
        return {
          cancel: () => void (cancelled = true),
          promise: (async () => {
            const cfg = (await p.optionalContentConfigPromise) as Iterable<[string, { name: string | null; visible: boolean }]> | undefined
            if (cancelled) throw Object.assign(new Error('Rendering cancelled'), { name: 'RenderingCancelledException' })
            const layers: Record<string, boolean> = {}
            for (const [, g] of cfg ?? []) layers[g.name ?? ''] = g.visible
            renders.push({ page: n, canvas: p.canvas ?? null, layers })
          })(),
        }
      }
      return page
    }
    return doc
  })
  return { promise, destroy: () => task.destroy() }
}

export class TextLayer {
  textDivs: HTMLElement[] = []
  textContentItemsStr: string[] = []
  private source: { items: { str?: string }[] }
  private container: HTMLElement
  constructor({ textContentSource, container }: { textContentSource: { items: { str?: string }[] }; container: HTMLElement; viewport: unknown }) {
    this.source = textContentSource
    this.container = container
  }
  async render(): Promise<void> {
    for (const it of this.source.items) {
      const span = document.createElement('span')
      span.textContent = it.str ?? ''
      this.container.append(span)
      this.textDivs.push(span)
      this.textContentItemsStr.push(it.str ?? '')
    }
  }
  cancel(): void {}
}

interface LinkServiceLike {
  addLinkAttributes(a: HTMLAnchorElement, url: string, newWindow?: boolean): void
  getDestinationHash(dest: unknown): string
  goToDestination(dest: unknown): unknown
}

export class AnnotationLayer {
  private div: HTMLElement
  private linkService: LinkServiceLike
  constructor({ div, linkService }: { div: HTMLElement; linkService: LinkServiceLike; page: unknown; viewport: unknown }) {
    this.div = div
    this.linkService = linkService
  }
  async render({ annotations }: { annotations: { id: string; subtype: string; url?: string; newWindow?: boolean; dest?: unknown }[] }): Promise<void> {
    for (const data of annotations) {
      if (data.subtype === 'Popup') continue
      const section = document.createElement('section')
      section.setAttribute('data-annotation-id', data.id)
      if (data.subtype === 'Link') {
        section.className = 'linkAnnotation'
        const a = document.createElement('a')
        if (data.url) this.linkService.addLinkAttributes(a, data.url, data.newWindow)
        else if (data.dest) {
          a.href = this.linkService.getDestinationHash(data.dest)
          a.onclick = () => {
            void this.linkService.goToDestination(data.dest)
            return false
          }
        }
        section.append(a)
      }
      this.div.append(section)
    }
  }
}
