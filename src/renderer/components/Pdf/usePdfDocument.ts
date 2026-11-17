// The PDF on screen: its bytes from the room's server (pdf-bytes: so a
// viewer in B's room reads B's file; the hosted web client reads the raw
// bytes over the transfer door, with progress), opened by pdf.js, and read
// again when any writer saves a new version. A new version keeps the old
// document on screen until the new one is in; a new file starts clean.

import { useEffect, useRef, useState } from 'react'
import * as pdfjsLib from 'pdfjs-dist'
import { useRoom } from '@/components/Room/RoomContext'
import { useDocVersion } from '@/components/FileViewerPane/useDocVersion'
import { bytesToBinaryString, loadPdfBytes } from '@/components/Pdf/pdf-bytes'

// One pdf.js worker for every PDF tab: ours (pdf.worker.ts), so the Map
// polyfill is in it before pdf.js's worker code runs.
if (typeof Worker !== 'undefined' && !pdfjsLib.GlobalWorkerOptions.workerPort) {
  pdfjsLib.GlobalWorkerOptions.workerPort = new Worker(new URL('./pdf.worker.ts', import.meta.url), { type: 'module', name: 'k2-pdf' })
}

export type PdfDoc = pdfjsLib.PDFDocumentProxy

export interface PdfDocState {
  doc: PdfDoc | null
  /** The file's bytes as a binary string (raw-annots reads /NM from it). */
  binary: string
  loading: boolean
  error: string | null
  /** 0..1 while the bytes arrive, when the transport can tell (the hosted
   *  web client); null otherwise. */
  progress: number | null
}

export function usePdfDocument(filePath: string): PdfDocState {
  const scope = useRoom().scope
  const [state, setState] = useState<PdfDocState>({ doc: null, binary: '', loading: true, error: null, progress: null })
  // The file the document on screen came from.
  const shownPathRef = useRef<string | null>(null)
  const version = useDocVersion(scope, filePath)

  useEffect(() => {
    let cancelled = false
    const ac = new AbortController()
    const reread = shownPathRef.current === filePath
    if (!reread) {
      shownPathRef.current = null
      setState({ doc: null, binary: '', loading: true, error: null, progress: null })
    }
    // Progress repaints at most once a percent.
    let shownPct = -1
    const onProgress = (ratio: number): void => {
      const pct = Math.floor(ratio * 100)
      if (cancelled || reread || pct === shownPct) return
      shownPct = pct
      setState((s) => (s.loading ? { ...s, progress: ratio } : s))
    }
    void (async () => {
      try {
        const data = await loadPdfBytes(scope, filePath, { onProgress, signal: ac.signal })
        if (cancelled) return
        // Before pdf.js takes the buffer (it moves it to its worker).
        const binary = bytesToBinaryString(data)
        const doc = await pdfjsLib.getDocument({ data }).promise
        if (cancelled) return
        shownPathRef.current = filePath
        setState({ doc, binary, loading: false, error: null, progress: null })
      } catch (err) {
        if (cancelled) return
        shownPathRef.current = null
        setState({ doc: null, binary: '', loading: false, error: err instanceof Error ? err.message : String(err), progress: null })
      }
    })()
    return () => {
      cancelled = true
      // A tab closed (or another file) stops its download.
      ac.abort()
    }
    // `scope` is read live, as before.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, version])

  return state
}
