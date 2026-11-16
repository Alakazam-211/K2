// One page of the PDF, as paper on the desk: the canvas pdf.js draws (with
// this tab's layers and the annotations' own appearances), then pdf.js's
// text layer (selectable, transparent text in place) and annotation layer
// (links, and a hit area per comment or markup) above it.
//   - The canvas redraws for a new page, zoom or layer choice; the two
//     layers rebuild only for a new page or zoom.
//   - Clicking a listed annotation opens its popover (PdfTab owns which);
//     a link does what a link does (link-service.ts).
//   - Find's matches on this page are marked in the text layer, the current
//     one scrolled into view.
//   - pdf.js's own note popups stay hidden: the popover is K2's.

import { useEffect, useRef, useState } from 'react'
import * as pdfjsLib from 'pdfjs-dist'
import type { PdfAnnot } from '@/lib/pdf/annotations'
import type { OcConfig } from '@/lib/pdf/layers'
import type { PdfLinkService } from '@/lib/pdf/link-service'
import { pageTextOf } from '@/lib/pdf/text-cache'
import type { PdfDoc } from './usePdfDocument'

/** A match's part in one text item; `current` marks the match Find is on. */
export interface Mark {
  item: number
  from: number
  to: number
  current: boolean
}

interface Viewport {
  width: number
  height: number
  userUnit?: number
  clone(o: { dontFlip: boolean }): Viewport
  convertToViewportRectangle(r: number[]): number[]
}

export function PdfPageView({
  doc,
  pageNumber,
  zoom,
  cfg,
  cfgKey,
  linkService,
  annots,
  marks,
  selectedId,
  onAnnotClick,
  overlay,
}: {
  doc: PdfDoc
  pageNumber: number
  /** Null until the fit is known (the page's size, the desk's): nothing draws. */
  zoom: number | null
  cfg: OcConfig | null
  /** Changes whenever a layer does: the canvas redraws. */
  cfgKey: number
  linkService: PdfLinkService
  /** This page's listed annotations. */
  annots: readonly PdfAnnot[]
  marks: readonly Mark[]
  /** The annotation whose popover is open (pdf.js id). */
  selectedId: string | null
  onAnnotClick: (a: PdfAnnot) => void
  /** Drawn over the page, in its coordinates (the popover). */
  overlay?: (vp: Viewport) => React.ReactNode
}): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const textRef = useRef<HTMLDivElement>(null)
  const annotRef = useRef<HTMLDivElement>(null)
  const renderTask = useRef<{ cancel(): void } | null>(null)
  const [vp, setVp] = useState<Viewport | null>(null)
  // Bumps when the text layer is in, so marks apply to its spans.
  const [textKey, setTextKey] = useState(0)
  // Bumps when the annotation layer is in.
  const [annotKey, setAnnotKey] = useState(0)
  const textItems = useRef<{ divs: HTMLElement[]; strs: string[] }>({ divs: [], strs: [] })
  const annotsRef = useRef(annots)
  annotsRef.current = annots

  // The canvas.
  useEffect(() => {
    if (zoom === null) return
    const z = zoom
    let cancelled = false
    void (async () => {
      renderTask.current?.cancel()
      const page = await doc.getPage(pageNumber)
      if (cancelled) return
      const css = page.getViewport({ scale: z }) as unknown as Viewport
      setVp((old) => (old && old.width === css.width && old.height === css.height ? old : css))
      const dpr = window.devicePixelRatio || 1
      const viewport = page.getViewport({ scale: z * dpr })
      const canvas = canvasRef.current
      if (!canvas) return
      canvas.width = Math.floor(viewport.width)
      canvas.height = Math.floor(viewport.height)
      canvas.style.width = `${css.width}px`
      canvas.style.height = `${css.height}px`
      const task = page.render({ canvas, viewport, ...(cfg ? { optionalContentConfigPromise: Promise.resolve(cfg as never) } : {}) })
      renderTask.current = task
      try {
        await task.promise
      } catch (err) {
        if ((err as Error)?.name === 'RenderingCancelledException') return
        if (!cancelled) console.error('[pdf-viewer] Render error:', err)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [doc, pageNumber, zoom, cfg, cfgKey])

  // The text layer and the annotation layer.
  useEffect(() => {
    let cancelled = false
    const textDiv = textRef.current
    const annotDiv = annotRef.current
    if (!textDiv || !annotDiv || zoom === null) return
    const z = zoom
    textDiv.replaceChildren()
    annotDiv.replaceChildren()
    textItems.current = { divs: [], strs: [] }
    let textLayer: { cancel(): void } | null = null
    void (async () => {
      const page = await doc.getPage(pageNumber)
      if (cancelled) return
      const viewport = page.getViewport({ scale: z })
      const { content } = await pageTextOf(doc, pageNumber)
      if (cancelled) return
      const tl = new pdfjsLib.TextLayer({ textContentSource: content as never, container: textDiv, viewport })
      textLayer = tl
      try {
        await tl.render()
      } catch (err) {
        if (!cancelled) console.error('[pdf-viewer] Text layer error:', err)
        return
      }
      if (cancelled) return
      textItems.current = { divs: [...tl.textDivs], strs: [...tl.textContentItemsStr] }
      setTextKey((k) => k + 1)

      const annotations = await page.getAnnotations({ intent: 'display' })
      if (cancelled) return
      const layer = new pdfjsLib.AnnotationLayer({
        div: annotDiv,
        page,
        viewport: viewport.clone({ dontFlip: true }),
        linkService: linkService as never,
        accessibilityManager: null,
        annotationCanvasMap: null,
        annotationEditorUIManager: null,
        structTreeLayer: null,
        commentManager: null,
        annotationStorage: undefined as never,
      } as never)
      try {
        await layer.render({ annotations, linkService, renderForms: false, imageResourcesPath: '', enableScripting: false, hasJSActions: false, fieldObjects: null } as never)
      } catch (err) {
        if (!cancelled) console.error('[pdf-viewer] Annotation layer error:', err)
        return
      }
      if (cancelled) return
      setAnnotKey((k) => k + 1)
    })()
    return () => {
      cancelled = true
      textLayer?.cancel()
    }
  }, [doc, pageNumber, zoom, linkService])

  // Which sections are listed annotations, and which is selected.
  useEffect(() => {
    const div = annotRef.current
    if (!div) return
    markSections(div, annots)
    for (const s of div.querySelectorAll<HTMLElement>('section[data-annotation-id]')) {
      if (s.getAttribute('data-annotation-id') === selectedId) s.setAttribute('data-selected', '')
      else s.removeAttribute('data-selected')
    }
  }, [selectedId, annotKey, annots])

  // Find's marks.
  useEffect(() => {
    const { divs, strs } = textItems.current
    divs.forEach((d, i) => {
      if (d.querySelector('.pdf-hl')) d.textContent = strs[i]
    })
    let current: HTMLElement | null = null
    const byItem = new Map<number, Mark[]>()
    for (const m of marks) byItem.set(m.item, [...(byItem.get(m.item) ?? []), m])
    for (const [i, ms] of byItem) {
      const d = divs[i]
      const s = strs[i]
      if (!d || s === undefined) continue
      d.replaceChildren()
      let at = 0
      for (const m of [...ms].sort((a, b) => a.from - b.from)) {
        if (m.from > at) d.append(s.slice(at, m.from))
        const hl = document.createElement('span')
        hl.className = 'pdf-hl'
        hl.textContent = s.slice(m.from, m.to)
        if (m.current) {
          hl.setAttribute('data-current', '')
          current ??= hl
        }
        d.append(hl)
        at = m.to
      }
      if (at < s.length) d.append(s.slice(at))
    }
    current?.scrollIntoView?.({ block: 'center', inline: 'center' })
  }, [marks, textKey])

  const onClick = (e: React.MouseEvent): void => {
    const sec = (e.target as Element).closest?.('section[data-annotation-id]')
    if (!sec || sec.querySelector('a')) return
    const a = annotsRef.current.find((x) => x.id === sec.getAttribute('data-annotation-id'))
    if (a) onAnnotClick(a)
  }

  const userUnit = vp?.userUnit ?? 1
  return (
    <div
      data-testid="pdf-sheet"
      data-page={pageNumber}
      className="pdf-sheet"
      style={{ width: vp?.width, height: vp?.height, ['--scale-factor' as string]: zoom ?? 1, ['--user-unit' as string]: userUnit, ['--total-scale-factor' as string]: (zoom ?? 1) * userUnit }}
    >
      <canvas ref={canvasRef} data-testid="pdf-canvas" className="pdf-canvas" />
      <div ref={textRef} data-testid="pdf-text-layer" className="textLayer" />
      <div ref={annotRef} data-testid="pdf-annotation-layer" className="annotationLayer" onClick={onClick} />
      {vp && overlay ? overlay(vp) : null}
    </div>
  )
}

/** Mark the sections that are listed annotations (a pointer, a ring when selected); a comment without its own appearance gets K2's note mark. */
function markSections(div: HTMLElement, annots: readonly PdfAnnot[]): void {
  const byId = new Map(annots.map((a) => [a.id, a]))
  for (const s of div.querySelectorAll<HTMLElement>('section[data-annotation-id]')) {
    const a = byId.get(s.getAttribute('data-annotation-id') ?? '')
    if (!a) continue
    s.setAttribute('data-pdf-annot', a.subtype)
    s.setAttribute('aria-label', `${a.label} by ${a.author}`)
    if (a.subtype === 'Text' && !a.hasAppearance) s.setAttribute('data-pdf-note', '')
  }
}
