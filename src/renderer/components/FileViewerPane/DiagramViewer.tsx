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
        // Dynamic import keeps mermaid out of the cold path until a
        // .mermaid/.mmd tab is opened.
        const mermaid = (await import('mermaid')).default
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
