import { useState, useEffect } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import mammoth from 'mammoth'

interface DocxViewerProps {
  filePath: string
}

export function DocxViewer({ filePath }: DocxViewerProps): React.JSX.Element {
  const [html, setHtml] = useState<string>('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false

    async function loadDocx(): Promise<void> {
      setLoading(true)
      setError(null)

      try {
        // Read the file as base64 via the daemon and decode locally.
        const r = await daemonCliGet<{ base64: string }>('fs/read-binary', { path: filePath })

        if (cancelled) return

        const binary = atob(r.base64)
        const uint8 = new Uint8Array(binary.length)
        for (let i = 0; i < binary.length; i++) uint8[i] = binary.charCodeAt(i)
        const arrayBuffer = uint8.buffer

        // Convert DOCX to HTML using mammoth
        const result = await mammoth.convertToHtml({ arrayBuffer })

        if (cancelled) return

        setHtml(result.value)

        // Log any warnings from mammoth for debugging
        if (result.messages.length > 0) {
          console.warn('[docx-viewer] Mammoth warnings:', result.messages)
        }
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
      } finally {
        if (!cancelled) {
          setLoading(false)
        }
      }
    }

    loadDocx()

    return () => {
      cancelled = true
    }
  }, [filePath])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-sm">
        Converting document...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[#0a0a0a]">
        <span className="text-red-400 text-sm">Failed to load document</span>
        <span className="text-xs text-[var(--color-text-muted)] max-w-md text-center px-4">
          {error}
        </span>
      </div>
    )
  }

  return (
    <div
      className="docx-body p-6 overflow-y-auto h-full bg-[#0a0a0a]"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
