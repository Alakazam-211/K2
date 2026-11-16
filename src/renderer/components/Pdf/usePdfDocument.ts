// The PDF on screen: its bytes from the room's server (fs/read-binary, so a
// viewer in B's room reads B's file), opened by pdf.js, and read again when
// any writer saves a new version. A new version keeps the old document on
// screen until the new one is in; a new file starts clean.

import { useEffect, useRef, useState } from 'react'
import * as pdfjsLib from 'pdfjs-dist'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useRoom } from '@/components/Room/RoomContext'
import { useDocVersion } from '@/components/FileViewerPane/useDocVersion'

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
}

export function usePdfDocument(filePath: string): PdfDocState {
  const scope = useRoom().scope
  const [state, setState] = useState<PdfDocState>({ doc: null, binary: '', loading: true, error: null })
  // The file the document on screen came from.
  const shownPathRef = useRef<string | null>(null)
  const version = useDocVersion(scope, filePath)

  useEffect(() => {
    let cancelled = false
    const reread = shownPathRef.current === filePath
    if (!reread) {
      shownPathRef.current = null
      setState({ doc: null, binary: '', loading: true, error: null })
    }
    void (async () => {
      try {
        // The daemon sends base64; atob's binary string tops out far above a 50 MB drawing.
        const r = await daemonCliGet<{ base64: string }>(scope, 'fs/read-binary', { path: filePath })
        const binary = atob(r.base64)
        const data = new Uint8Array(binary.length)
        for (let i = 0; i < binary.length; i++) data[i] = binary.charCodeAt(i)
        const doc = await pdfjsLib.getDocument({ data }).promise
        if (cancelled) return
        shownPathRef.current = filePath
        setState({ doc, binary, loading: false, error: null })
      } catch (err) {
        if (cancelled) return
        shownPathRef.current = null
        setState({ doc: null, binary: '', loading: false, error: err instanceof Error ? err.message : String(err) })
      }
    })()
    return () => {
      cancelled = true
    }
    // `scope` is read live, as before.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, version])

  return state
}
