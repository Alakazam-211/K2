// The PDF tab (P1, single player), in the editors' frame on the live shell:
// one header (the file's tile and name, then page, zoom and Find on the
// right, and the side-panel toggle), Find docked under it, the page as white
// paper on the desk, the status line (page N of M, zoom, layers visible), and
// the side panel (Layers, Annotations, Pages) sliding like the drawers.
// Everything here is this tab's view of the file: which layers show, the
// page, the zoom. Nothing is written to the file.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import * as pdfjsLib from 'pdfjs-dist'
import { ChevronLeftIcon, ChevronRightIcon, MinusIcon, PlusIcon, SearchIcon } from 'lucide-react'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { LiveHeader } from '@/components/LiveView/shell/LiveHeader'
import { LiveTabFrame } from '@/components/LiveView/shell/LiveTabFrame'
import { useTabWidth } from '@/components/LiveView/shell/useLiveShell'
import { EDITOR_CSS } from '@/components/Editor/editorCss'
import { LIVE_CSS } from '@/components/Editor/live/liveCss'
import { EditorStatusLine } from '@/components/Editor/EditorStatusLine'
import { EDITOR_NARROW_BELOW, fileNameOf } from '@/components/Editor/EditorFrame'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import { isEditableTarget } from '@/lib/editable-target'
import { readAnnotations, type PdfAnnot } from '@/lib/pdf/annotations'
import { findCount, findMatches, matchSegments, type PageText } from '@/lib/pdf/find'
import { countCoverage, coverageSummary, layerView, type Coverage } from '@/lib/pdf/layer-coverage'
import { keepChoices, readLayers, setAllLayers, setLayer, type OcConfig } from '@/lib/pdf/layers'
import { makeLinkService } from '@/lib/pdf/link-service'
import { scanAnnotDicts } from '@/lib/pdf/raw-annots'
import { allPageText } from '@/lib/pdf/text-cache'
import { MAX_ZOOM, MIN_ZOOM, fitScale, zoomStep, type ZoomMode } from '@/lib/pdf/zoom'
import { AnnotPopover } from './AnnotPopover'
import { PDF_CSS } from './pdfCss'
import { PdfFindBar } from './PdfFindBar'
import { PdfPageView, type Mark } from './PdfPageView'
import { PdfPanel, type PdfPanelTab } from './PdfPanel'
import { PdfZoomMenu, type ZoomChoice } from './PdfZoomMenu'
import type { PdfDoc } from './usePdfDocument'

const PANEL_LABEL = 'layers, annotations and pages'

function IconBtn({ label, onClick, disabled, pressed, children }: { label: string; onClick: () => void; disabled?: boolean; pressed?: boolean; children: React.ReactNode }): React.JSX.Element {
  return (
    <button type="button" data-k2-focus="" data-k2-chamfer="control" aria-label={label} title={label} aria-pressed={pressed} disabled={disabled} onClick={onClick} className="lv-icon-btn pdf-head-btn no-drag">
      {children}
    </button>
  )
}

export function PdfTab({ doc, binary, filePath, width }: { doc: PdfDoc; binary: string; filePath: string; width?: number }): React.JSX.Element {
  const [root, tabWidth] = useTabWidth(width)
  const narrow = tabWidth < EDITOR_NARROW_BELOW
  const numPages = doc.numPages
  const [page, setPage] = useState(1)
  // Zoom: Fit page until the person picks (lib/pdf/zoom.ts); their pick stays for this tab.
  const [zoomMode, setZoomMode] = useState<ZoomMode>('page')
  const [customZoom, setCustomZoom] = useState(1)
  const desk = useRef<HTMLDivElement>(null)
  const [deskBox, setDeskBox] = useState<{ w: number; h: number } | null>(null)
  useLayoutEffect(() => {
    const el = desk.current
    if (!el) return
    const read = (): void => setDeskBox((b) => (b && b.w === el.clientWidth && b.h === el.clientHeight ? b : { w: el.clientWidth, h: el.clientHeight }))
    read()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  // This page's size at 100% (sheets in a set can differ).
  const [base, setBase] = useState<{ page: number; w: number; h: number } | null>(null)
  useEffect(() => {
    let live = true
    void doc.getPage(page).then((p) => {
      const v = p.getViewport({ scale: 1 })
      if (live) setBase({ page, w: v.width, h: v.height })
    })
    return () => void (live = false)
  }, [doc, page])
  // Until the page's size is known, a fit has nothing to draw at.
  const fitted = zoomMode === 'custom' ? customZoom : base && deskBox ? fitScale(zoomMode, base, deskBox) : null
  const zoom = fitted ?? 1
  const pageRef = useRef(page)
  pageRef.current = page
  const goToPage = useCallback((n: number) => setPage(Math.max(1, Math.min(n, numPages))), [numPages])
  // A new version can be shorter.
  useEffect(() => setPage((p) => Math.max(1, Math.min(p, numPages))), [numPages])

  // Layers: one config per tab, from the PDF's default; this tab's choices survive a new version (by name).
  const [cfg, setCfg] = useState<OcConfig | null>(null)
  const [cfgKey, setCfgKey] = useState(0)
  const choices = useRef(new Map<string, boolean>())
  useEffect(() => {
    let live = true
    void doc.getOptionalContentConfig().then((c) => {
      if (!live) return
      const oc = c as unknown as OcConfig
      keepChoices(oc, choices.current)
      setCfg(oc)
      setCfgKey((k) => k + 1)
    })
    return () => void (live = false)
  }, [doc])
  const layers = useMemo(() => readLayers(cfg), [cfg, cfgKey])
  const layersChanged = useCallback(() => {
    if (!cfg) return
    for (const l of readLayers(cfg)) choices.current.set(l.name, l.visible)
    setCfgKey((k) => k + 1)
  }, [cfg])
  const onLayer = (id: string, visible: boolean): void => {
    if (!cfg) return
    setLayer(cfg, id, visible)
    layersChanged()
  }
  const onAllLayers = (visible: boolean): void => {
    if (!cfg) return
    setAllLayers(cfg, visible)
    layersChanged()
  }

  // What each layer covers, page by page in the background.
  const [coverage, setCoverage] = useState<Coverage | null>(null)
  const hasLayers = layers.length > 0
  useEffect(() => {
    if (!hasLayers) return
    let live = true
    const counts = new Map<string, number>()
    setCoverage({ scanned: 0, total: numPages, counts: new Map() })
    void (async () => {
      for (let n = 1; n <= numPages && live; n++) {
        try {
          const p = await doc.getPage(n)
          countCoverage(await p.getOperatorList(), pdfjsLib.OPS as unknown as Record<string, number>, counts)
          // pdf.js keeps each operator list; a long drawing set shouldn't hold them all.
          if (n !== pageRef.current) p.cleanup()
        } catch (err) {
          if (!live) return
          console.error('[pdf-viewer] Layer check failed on page', n, err)
        }
        if (live) setCoverage({ scanned: n, total: numPages, counts: new Map(counts) })
      }
    })()
    return () => void (live = false)
  }, [doc, numPages, hasLayers])
  const view = useMemo(() => layerView(layers, coverage), [layers, coverage])

  // Annotations, all pages.
  const [annots, setAnnots] = useState<PdfAnnot[] | null>(null)
  useEffect(() => {
    let live = true
    setAnnots(null)
    void readAnnotations(doc, scanAnnotDicts(binary)).then(
      (a) => live && setAnnots(a),
      (err) => {
        if (!live) return
        console.error('[pdf-viewer] Reading annotations failed:', err)
        setAnnots([])
      },
    )
    return () => void (live = false)
  }, [doc, binary])
  const [popKey, setPopKey] = useState<string | null>(null)
  const pop = annots?.find((a) => a.key === popKey) ?? null
  const closePop = useCallback(() => setPopKey(null), [])
  const openAnnot = (a: PdfAnnot): void => {
    goToPage(a.page)
    setPopKey(a.key)
  }

  const cfgRef = useRef(cfg)
  cfgRef.current = cfg
  const layersChangedRef = useRef(layersChanged)
  layersChangedRef.current = layersChanged
  const linkService = useMemo(
    () =>
      makeLinkService({
        doc,
        page: () => pageRef.current,
        goToPage: (n) => {
          setPopKey(null)
          setPage(Math.max(1, Math.min(n, doc.numPages)))
        },
        setOCGState: (action) => {
          ;(cfgRef.current as unknown as { setOCGState?: (a: unknown) => void } | null)?.setOCGState?.(action)
          layersChangedRef.current()
        },
      }),
    [doc],
  )
  // Panel.
  const [panelOpen, setPanelOpen] = useState(false)
  const [panelTab, setPanelTab] = useState<PdfPanelTab>('layers')

  // Find.
  const [findOpen, setFindOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [matchCase, setMatchCase] = useState(false)
  const [texts, setTexts] = useState<PageText[] | null>(null)
  const [current, setCurrent] = useState(0)
  useEffect(() => setTexts(null), [doc])
  useEffect(() => {
    if (!findOpen || texts) return
    let live = true
    void allPageText(doc).then((t) => live && setTexts(t))
    return () => void (live = false)
  }, [findOpen, texts, doc])
  const matches = useMemo(() => (findOpen && texts ? findMatches(texts, query, { matchCase }) : []), [findOpen, texts, query, matchCase])
  // A new query starts at the first match on or after this page.
  useEffect(() => {
    const at = matches.findIndex((m) => m.page >= pageRef.current)
    const i = at < 0 ? 0 : at
    setCurrent(i)
    if (matches[i] && matches[i].page !== pageRef.current) setPage(matches[i].page)
  }, [matches])
  const goMatch = (i: number): void => {
    if (matches.length === 0) return
    const n = (i + matches.length) % matches.length
    setCurrent(n)
    goToPage(matches[n].page)
  }
  const marks = useMemo<Mark[]>(() => {
    const t = texts?.[page - 1]
    if (!t) return []
    return matches.flatMap((m, i) => (m.page === page ? matchSegments(t, m).map((s) => ({ ...s, current: i === current })) : []))
  }, [matches, texts, page, current])
  const openFind = (): void => setFindOpen(true)
  const closeFind = (): void => {
    setFindOpen(false)
    root.current?.querySelector<HTMLElement>('[data-pdf-find-toggle] button')?.focus()
  }

  const zoomRef = useRef(zoom)
  zoomRef.current = zoom
  const setCustom = useCallback((z: number) => {
    setZoomMode('custom')
    setCustomZoom(z)
  }, [])
  const zoomIn = useCallback(() => setCustom(zoomStep(zoomRef.current, 1)), [setCustom])
  const zoomOut = useCallback(() => setCustom(zoomStep(zoomRef.current, -1)), [setCustom])
  const resetZoom = useCallback(() => setCustom(1), [setCustom])
  const chooseZoom = (c: ZoomChoice): void => (c === 'actual' ? setCustom(1) : setZoomMode(c))

  // Keys. The listener is on the window, so it hears keys meant for anything
  // else: a key typed into a field stays there, and a tab that isn't showing
  // (a hidden tab, the workspace under Settings) takes none.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.defaultPrevented) return
      const el = root.current
      if (!el || el.getClientRects().length === 0) return
      const mod = e.metaKey || e.ctrlKey
      if (mod && e.key.toLowerCase() === 'f') {
        e.preventDefault()
        setFindOpen(true)
        el.querySelector<HTMLInputElement>('[data-testid="pdf-find"] input')?.focus()
        return
      }
      if (isEditableTarget(e.target)) return
      if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
        e.preventDefault()
        goToPage(pageRef.current - 1)
      } else if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
        e.preventDefault()
        goToPage(pageRef.current + 1)
      } else if (mod && (e.key === '+' || e.key === '=')) {
        e.preventDefault()
        zoomIn()
      } else if (mod && e.key === '-') {
        e.preventDefault()
        zoomOut()
      } else if (mod && e.key === '0') {
        e.preventDefault()
        resetZoom()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [root, goToPage, zoomIn, zoomOut, resetZoom])

  // A new page starts at its top.
  useEffect(() => {
    desk.current?.scrollTo?.({ top: 0 })
  }, [page])

  const name = fileNameOf(filePath)
  const pct = `${Math.round(zoom * 100)}%`
  const pageAnnots = useMemo(() => (annots ?? []).filter((a) => a.page === page), [annots, page])
  const controls = (
    <div className="pdf-head-ctrls">
      <IconBtn label="Previous page" onClick={() => goToPage(page - 1)} disabled={page <= 1}>
        <ChevronLeftIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
      </IconBtn>
      <span data-testid="pdf-page-of" className="pdf-page-of type-row lv-num" aria-label={`Page ${page} of ${numPages}`}>
        {page} / {numPages}
      </span>
      <IconBtn label="Next page" onClick={() => goToPage(page + 1)} disabled={page >= numPages}>
        <ChevronRightIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
      </IconBtn>
      <span aria-hidden="true" className="pdf-head-sep" />
      <IconBtn label="Zoom out" onClick={zoomOut} disabled={zoom <= MIN_ZOOM}>
        <MinusIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
      </IconBtn>
      <PdfZoomMenu pct={pct} mode={zoomMode} zoom={zoom} onChoose={chooseZoom} />
      <IconBtn label="Zoom in" onClick={zoomIn} disabled={zoom >= MAX_ZOOM}>
        <PlusIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
      </IconBtn>
      <span aria-hidden="true" className="pdf-head-sep" />
      <span data-pdf-find-toggle="" className="contents">
        <IconBtn label="Find" pressed={findOpen} onClick={() => (findOpen ? closeFind() : openFind())}>
          <SearchIcon size={16} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
        </IconBtn>
      </span>
    </div>
  )

  return (
    <div ref={root} data-testid="pdf-tab" className="ed-tab pdf-tab flex h-full min-h-0 flex-col">
      <style>{EDITOR_CSS + LIVE_CSS + PDF_CSS}</style>
      <LiveTabFrame
        header={
          <LiveHeader
            fileName={name}
            fileTitle={filePath}
            fileGlyph={<SetiFileIcon name={name} size={16} />}
            sentence={null}
            right={controls}
            panelOpen={panelOpen}
            onTogglePanel={() => setPanelOpen((o) => !o)}
            panelLabel={PANEL_LABEL}
          />
        }
        findBar={
          findOpen ? (
            <PdfFindBar
              query={query}
              onQuery={setQuery}
              matchCase={matchCase}
              onMatchCase={() => setMatchCase((c) => !c)}
              count={findCount(query, matches, current)}
              hasMatches={matches.length > 0}
              reading={texts === null}
              onNext={() => goMatch(current + 1)}
              onPrev={() => goMatch(current - 1)}
              onClose={closeFind}
            />
          ) : null
        }
        hero={
          <div ref={desk} data-testid="pdf-desk" className="pdf-desk">
            <div className="pdf-desk-inner">
              <PdfPageView
                doc={doc}
                pageNumber={page}
                zoom={fitted}
                cfg={cfg}
                cfgKey={cfgKey}
                linkService={linkService}
                annots={pageAnnots}
                marks={marks}
                selectedId={pop && pop.page === page ? pop.id : null}
                onAnnotClick={(a) => setPopKey(a.key)}
                overlay={(vp) => (pop && pop.page === page ? <AnnotPopover annot={pop} box={vp.convertToViewportRectangle(pop.rect)} pageW={vp.width} pageH={vp.height} onClose={closePop} /> : null)}
              />
            </div>
          </div>
        }
        status={
          <EditorStatusLine
            items={[
              { id: 'page', node: `Page ${page} of ${numPages}` },
              // Narrow, the header's zoom says it; the status line keeps to the page's column.
              ...(narrow ? [] : [{ id: 'zoom', node: pct }]),
              ...(layers.length ? [{ id: 'layers', node: view.checking ? `${layers.filter((l) => l.visible).length} of ${layers.length} layers` : coverageSummary(view) }] : []),
            ]}
            toggles={[]}
            facts={[]}
            narrow={narrow}
          />
        }
        panel={
          <PdfPanel
            tab={panelTab}
            onTab={setPanelTab}
            doc={doc}
            layers={view}
            coverage={coverage}
            onLayer={onLayer}
            onAllLayers={onAllLayers}
            annots={annots}
            selectedKey={popKey}
            onOpenAnnot={openAnnot}
            cfg={cfg}
            cfgKey={cfgKey}
            page={page}
            onGo={goToPage}
          />
        }
        panelOpen={panelOpen}
        panelMode="reflow"
        panelLabel="Layers, annotations and pages"
        narrow={narrow}
        sample={false}
        onClosePanel={() => setPanelOpen(false)}
      />
    </div>
  )
}
