// The preview of a text file that has one: CSV/TSV as a table, Mermaid and
// diagram sources drawn, a DXF drawing, an HTML page. Today's file view shows
// it as Preview; the new code editor shows the same thing as its Preview
// (until E6 restyles them). Each viewer loads lazily, as before.

import { Suspense, lazy } from 'react'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import { ViewerNote } from './ViewerNote'
import { dxfReadErrorNote, dxfReadsAsBytes } from './dxfReadError'
import type { FileCategory } from './fileCategory'

const CsvViewer = lazy(() => import('./CsvViewer').then((m) => ({ default: m.CsvViewer })))
const DiagramViewer = lazy(() => import('./DiagramViewer').then((m) => ({ default: m.DiagramViewer })))
// CAD previews (ported from the K2 Companion): dxf-parser loads with the viewer.
const DxfViewer = lazy(() => import('./DxfViewer').then((m) => ({ default: m.DxfViewer })))
const DxfHostBytesViewer = lazy(() => import('./DxfViewer').then((m) => ({ default: m.DxfHostBytesViewer })))

export type PreviewCategory = 'csv' | 'mermaid' | 'diagram' | 'cad2d' | 'html'

export function hasFilePreview(category: FileCategory): category is PreviewCategory {
  return category === 'csv' || category === 'mermaid' || category === 'diagram' || category === 'cad2d' || category === 'html'
}

const LOADING: Record<Exclude<PreviewCategory, 'html'>, string> = { csv: 'Loading table…', mermaid: 'Loading diagram…', diagram: 'Loading diagram…', cad2d: 'Loading drawing…' }

export function FilePreview(p: {
  category: PreviewCategory
  filePath: string
  /** The buffer (unsaved edits included): the table, diagram and drawing draw it. */
  text: string
  /** The file as saved: an HTML page shows this, as before. */
  saved: string
  /** The last read's refusal (a DXF too large or not UTF-8 is read as bytes). */
  readError: string | null
  contentRef?: React.Ref<HTMLDivElement>
}): React.JSX.Element {
  const { category, filePath, text } = p
  if (category === 'html') {
    // Render HTML in a sandboxed <iframe>, NOT via dangerouslySetInnerHTML.
    // HtmlFrame `scripted`: null-origin sandbox (`allow-scripts`, never
    // `allow-same-origin`) plus a per-frame CSP meta, so the page's inline
    // scripts run but it cannot reach the K2 app, the daemon on loopback, or
    // any plain-http host (prd-html-frame-csp-v1.md). White background so dark
    // app chrome doesn't bleed into light HTML.
    return (
      <div className="flex-1 overflow-hidden bg-white" ref={p.contentRef}>
        <HtmlFrame title={filePath.split('/').pop() || filePath} html={p.saved} profile="scripted" className="w-full h-full border-0 bg-white" />
      </div>
    )
  }
  return (
    <div className="flex-1 overflow-hidden" ref={p.contentRef}>
      <Suspense fallback={<div className="p-4 text-xs text-[var(--color-text-muted)]">{LOADING[category]}</div>}>
        {category === 'csv' ? (
          <CsvViewer content={text} filePath={filePath} />
        ) : category === 'mermaid' || category === 'diagram' ? (
          <DiagramViewer content={text} filePath={filePath} mode={category === 'mermaid' ? 'mermaid' : 'source'} />
        ) : p.readError && !p.saved && dxfReadsAsBytes(p.readError) ? (
          // Over read-file's 10 MB cap, or not UTF-8: read the bytes.
          <DxfHostBytesViewer filePath={filePath} />
        ) : p.readError && !p.saved ? (
          <ViewerNote tone="error" testId="dxf-read-error" {...dxfReadErrorNote(p.readError)} />
        ) : (
          <DxfViewer text={text} />
        )}
      </Suspense>
    </div>
  )
}
