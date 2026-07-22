// Diagram-oriented file previews for FileViewerPane.
//
// - .mermaid / .mmd → client-side Mermaid render (secure defaults)
// - .drawio / .dio / .puml / .plantuml / .dot / .gv → honest empty-state
//   plus a compact source peek (no PlantUML/Graphviz/diagrams.net server)
//
// Parent owns dual-mode Preview | Edit; this is Preview only.

import { useEffect, useId, useRef, useState } from 'react'
import { getFileExtension } from './fileCategory'

interface DiagramViewerProps {
  content: string
  filePath: string
  /** When true, run Mermaid; otherwise show source-oriented empty state. */
  mode: 'mermaid' | 'source'
}

function diagramKindLabel(filePath: string): string {
  const ext = getFileExtension(filePath)
  switch (ext) {
    case '.drawio':
    case '.dio':
      return 'Draw.io diagram'
    case '.puml':
    case '.plantuml':
      return 'PlantUML source'
    case '.dot':
    case '.gv':
      return 'Graphviz DOT source'
    case '.mermaid':
    case '.mmd':
      return 'Mermaid diagram'
    default:
      return 'Diagram source'
  }
}

function emptyStateHint(filePath: string): string {
  const ext = getFileExtension(filePath)
  if (ext === '.drawio' || ext === '.dio') {
    return 'Visual Draw.io embed is not included in v1. Switch to Edit to change the XML, or open the file in diagrams.net.'
  }
  if (ext === '.puml' || ext === '.plantuml') {
    return 'PlantUML server rendering is not bundled. Switch to Edit to change the source.'
  }
  if (ext === '.dot' || ext === '.gv') {
    return 'Graphviz is not bundled for in-app render. Switch to Edit to change the DOT source.'
  }
  return 'No visual renderer for this diagram type. Switch to Edit for the raw source.'
}

export function DiagramViewer({
  content,
  filePath,
  mode,
}: DiagramViewerProps): React.JSX.Element {
  if (mode === 'mermaid') {
    return <MermaidPreview content={content} filePath={filePath} />
  }
  return <SourceDiagramPreview content={content} filePath={filePath} />
}

function SourceDiagramPreview({
  content,
  filePath,
}: {
  content: string
  filePath: string
}): React.JSX.Element {
  const label = diagramKindLabel(filePath)
  const peek = content.length > 4000 ? `${content.slice(0, 4000)}\n…` : content

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg)]">
      <div className="flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-4 py-3">
        <p className="text-sm font-medium text-[var(--color-text-primary)]">{label}</p>
        <p className="mt-1 max-w-xl text-xs text-[var(--color-text-muted)]">
          {emptyStateHint(filePath)}
        </p>
      </div>
      <pre className="flex-1 overflow-auto p-4 text-[11px] font-mono leading-relaxed text-[var(--color-text-secondary)] whitespace-pre-wrap break-words">
        {peek || '(empty file)'}
      </pre>
    </div>
  )
}

/** Mermaid global after loading the vendored UMD/min build. */
type MermaidApi = {
  initialize: (config: Record<string, unknown>) => void
  render: (
    id: string,
    text: string,
  ) => Promise<{ svg: string; bindFunctions?: (el: Element) => void }>
}

let mermaidLoadPromise: Promise<MermaidApi> | null = null

/**
 * Load `public/vendor/mermaid.min.js` once. Avoids bundling mermaid
 * through Rolldown (panic on unicode in chunk hashes, Vite 8).
 */
function loadMermaidGlobal(): Promise<MermaidApi> {
  if (typeof window === 'undefined') {
    return Promise.reject(new Error('Mermaid requires a browser environment'))
  }
  const w = window as unknown as { mermaid?: MermaidApi }
  if (w.mermaid) return Promise.resolve(w.mermaid)
  if (mermaidLoadPromise) return mermaidLoadPromise

  mermaidLoadPromise = new Promise<MermaidApi>((resolve, reject) => {
    const existing = document.querySelector<HTMLScriptElement>(
      'script[data-k2-mermaid]',
    )
    if (existing && w.mermaid) {
      resolve(w.mermaid)
      return
    }
    const script = document.createElement('script')
    script.dataset.k2Mermaid = '1'
    // Vite base is relative for Tauri (`./`); public files land at root.
    script.src = `${import.meta.env.BASE_URL}vendor/mermaid.min.js`
    script.async = true
    script.onload = () => {
      const api = (window as unknown as { mermaid?: MermaidApi }).mermaid
      if (!api) {
        mermaidLoadPromise = null
        reject(new Error('mermaid global missing after script load'))
        return
      }
      resolve(api)
    }
    script.onerror = () => {
      mermaidLoadPromise = null
      reject(new Error('Failed to load vendored mermaid.min.js'))
    }
    document.head.appendChild(script)
  })
  return mermaidLoadPromise
}

function MermaidPreview({
  content,
  filePath,
}: {
  content: string
  filePath: string
}): React.JSX.Element {
  const hostRef = useRef<HTMLDivElement>(null)
  const reactId = useId().replace(/:/g, '')
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let cancelled = false

    async function render(): Promise<void> {
      setLoading(true)
      setError(null)
      const host = hostRef.current
      if (host) host.innerHTML = ''

      const source = content.trim()
      if (!source) {
        if (!cancelled) {
          setError(null)
          setLoading(false)
          if (host) {
            host.innerHTML =
              '<p class="text-sm text-[var(--color-text-muted)]">Empty Mermaid file</p>'
          }
        }
        return
      }

      try {
        // Load vendored mermaid.min.js via script tag — NOT bundler
        // import('mermaid'). Vite 8/Rolldown panics hashing mermaid's
        // unicode-heavy chunks (hash_placeholder mid-char slice).
        // Public asset: src/renderer/public/vendor/mermaid.min.js
        const mermaid = await loadMermaidGlobal()
        mermaid.initialize({
          startOnLoad: false,
          // Do not execute raw HTML from diagram text.
          securityLevel: 'strict',
          theme: 'dark',
          fontFamily: 'ui-sans-serif, system-ui, sans-serif',
        })

        const id = `mermaid-${reactId}-${Math.random().toString(36).slice(2, 9)}`
        const { svg, bindFunctions } = await mermaid.render(id, source)
        if (cancelled) return
        if (host) {
          host.innerHTML = svg
          // bindFunctions is optional; used for interactive diagrams.
          bindFunctions?.(host)
        }
        setLoading(false)
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
        setLoading(false)
        if (host) host.innerHTML = ''
      }
    }

    void render()
    return () => {
      cancelled = true
    }
  }, [content, reactId, filePath])

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-bg)]">
      {loading && (
        <div className="flex-shrink-0 px-4 py-2 text-xs text-[var(--color-text-muted)]">
          Rendering Mermaid…
        </div>
      )}
      {error && (
        <div className="flex-shrink-0 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-4 py-2">
          <p className="text-xs text-[var(--color-status-error-soft)]">Mermaid render failed</p>
          <p className="mt-1 text-[11px] font-mono text-[var(--color-text-muted)] whitespace-pre-wrap">
            {error}
          </p>
          <p className="mt-1 text-[10px] text-[var(--color-text-muted)]">
            Switch to Edit to fix the source.
          </p>
        </div>
      )}
      <div
        ref={hostRef}
        className="flex flex-1 items-start justify-center overflow-auto p-6 [&_svg]:max-w-full"
      />
    </div>
  )
}
