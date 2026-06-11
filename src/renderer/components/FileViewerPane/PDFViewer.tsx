import { useState, useEffect, useRef, useCallback } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import * as pdfjsLib from 'pdfjs-dist'

pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
  'pdfjs-dist/build/pdf.worker.min.mjs',
  import.meta.url,
).toString()

// ── Types ────────────────────────────────────────────────────────────────

interface PDFViewerProps {
  filePath: string
}

const ZOOM_STEP = 0.25
const MIN_ZOOM = 0.5
const MAX_ZOOM = 4.0
const DEFAULT_ZOOM = 1.0

// ── Component ────────────────────────────────────────────────────────────

export function PDFViewer({ filePath }: PDFViewerProps): React.JSX.Element {
  const [pdfDoc, setPdfDoc] = useState<pdfjsLib.PDFDocumentProxy | null>(null)
  const [numPages, setNumPages] = useState(0)
  const [currentPage, setCurrentPage] = useState(1)
  const [zoom, setZoom] = useState(DEFAULT_ZOOM)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const canvasRef = useRef<HTMLCanvasElement>(null)
  const renderTaskRef = useRef<pdfjsLib.RenderTask | null>(null)

  // Load PDF document
  useEffect(() => {
    let cancelled = false

    async function loadPdf(): Promise<void> {
      setLoading(true)
      setError(null)
      setPdfDoc(null)
      setCurrentPage(1)

      try {
        // Daemon returns base64 — decode into a Uint8Array. Browser
        // `atob` is the smallest path; ~50 MB files top out the
        // implementation's V8 string limit headroom (~256 MB) by a
        // wide margin so no chunking needed here.
        const r = await daemonCliGet<{ base64: string }>('fs/read-binary', { path: filePath })
        const binary = atob(r.base64)
        const data = new Uint8Array(binary.length)
        for (let i = 0; i < binary.length; i++) data[i] = binary.charCodeAt(i)

        const doc = await pdfjsLib.getDocument({ data }).promise

        if (!cancelled) {
          setPdfDoc(doc)
          setNumPages(doc.numPages)
          setLoading(false)
        }
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : String(err)
          setError(message)
          setLoading(false)
        }
      }
    }

    loadPdf()
    return () => { cancelled = true }
  }, [filePath])

  // Render current page
  useEffect(() => {
    if (!pdfDoc || !canvasRef.current) return

    let cancelled = false

    async function renderPage(): Promise<void> {
      // Cancel any in-progress render
      if (renderTaskRef.current) {
        try {
          renderTaskRef.current.cancel()
        } catch {
          // ignore
        }
      }

      try {
        const page = await pdfDoc!.getPage(currentPage)
        if (cancelled) return

        const canvas = canvasRef.current!

        const viewport = page.getViewport({ scale: zoom * window.devicePixelRatio })
        canvas.width = viewport.width
        canvas.height = viewport.height
        canvas.style.width = `${viewport.width / window.devicePixelRatio}px`
        canvas.style.height = `${viewport.height / window.devicePixelRatio}px`

        const renderTask = page.render({
          canvas,
          viewport,
        })
        renderTaskRef.current = renderTask

        await renderTask.promise
      } catch (err) {
        // RenderingCancelledException is expected when navigating quickly
        if (err instanceof Error && err.message.includes('cancelled')) return
        if (!cancelled) {
          console.error('[pdf-viewer] Render error:', err)
        }
      }
    }

    renderPage()
    return () => { cancelled = true }
  }, [pdfDoc, currentPage, zoom])

  const goToPage = useCallback((page: number) => {
    setCurrentPage(Math.max(1, Math.min(page, numPages)))
  }, [numPages])

  const zoomIn = useCallback(() => {
    setZoom((z) => Math.min(z + ZOOM_STEP, MAX_ZOOM))
  }, [])

  const zoomOut = useCallback(() => {
    setZoom((z) => Math.max(z - ZOOM_STEP, MIN_ZOOM))
  }, [])

  const resetZoom = useCallback(() => {
    setZoom(DEFAULT_ZOOM)
  }, [])

  // Keyboard navigation
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent): void {
      if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
        e.preventDefault()
        goToPage(currentPage - 1)
      } else if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
        e.preventDefault()
        goToPage(currentPage + 1)
      } else if (e.key === '+' || e.key === '=') {
        if (e.metaKey || e.ctrlKey) {
          e.preventDefault()
          zoomIn()
        }
      } else if (e.key === '-') {
        if (e.metaKey || e.ctrlKey) {
          e.preventDefault()
          zoomOut()
        }
      } else if (e.key === '0') {
        if (e.metaKey || e.ctrlKey) {
          e.preventDefault()
          resetZoom()
        }
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [currentPage, goToPage, zoomIn, zoomOut, resetZoom])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-xs font-mono">
        Loading PDF...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 bg-[#0a0a0a]">
        <span className="text-red-400 text-xs font-mono">Failed to load PDF</span>
        <span className="text-[var(--color-text-muted)] text-[10px] font-mono max-w-[300px] text-center">
          {error}
        </span>
      </div>
    )
  }

  return (
    <div className="flex h-full w-full flex-col bg-[#0a0a0a]">
      {/* Controls bar */}
      <div className="flex items-center gap-3 border-b border-[var(--color-border)] bg-[#111111] px-3 py-1 flex-shrink-0">
        {/* Page navigation */}
        <div className="flex items-center gap-1">
          <button
            className="px-1.5 py-0.5 text-[10px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-default transition-colors"
            onClick={() => goToPage(currentPage - 1)}
            disabled={currentPage <= 1}
            title="Previous page"
          >
            <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </button>
          <span className="text-[10px] font-mono text-[var(--color-text-secondary)] select-none">
            {currentPage} / {numPages}
          </span>
          <button
            className="px-1.5 py-0.5 text-[10px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-default transition-colors"
            onClick={() => goToPage(currentPage + 1)}
            disabled={currentPage >= numPages}
            title="Next page"
          >
            <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <polyline points="9 18 15 12 9 6" />
            </svg>
          </button>
        </div>

        <div className="w-px h-3 bg-[var(--color-border)]" />

        {/* Zoom controls */}
        <div className="flex items-center gap-1">
          <button
            className="px-1.5 py-0.5 text-[10px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-default transition-colors"
            onClick={zoomOut}
            disabled={zoom <= MIN_ZOOM}
            title="Zoom out"
          >
            <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
          <button
            className="text-[10px] font-mono text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors select-none px-1"
            onClick={resetZoom}
            title="Reset zoom"
          >
            {Math.round(zoom * 100)}%
          </button>
          <button
            className="px-1.5 py-0.5 text-[10px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-default transition-colors"
            onClick={zoomIn}
            disabled={zoom >= MAX_ZOOM}
            title="Zoom in"
          >
            <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <line x1="12" y1="5" x2="12" y2="19" />
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
        </div>
      </div>

      {/* PDF canvas area */}
      <div className="flex-1 overflow-auto flex justify-center p-4 bg-[#0a0a0a]">
        <canvas
          ref={canvasRef}
          className="block shadow-[0_2px_8px_rgba(0,0,0,0.5)]"
        />
      </div>
    </div>
  )
}
