import { useState, useEffect, useCallback } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { daemonCliGet } from '@/lib/daemon-cli'
import { openOffOriginHttp } from '@/lib/open-off-origin-http'
import { useRoom } from '@/components/Room/RoomContext'
import mammoth from 'mammoth'
import { sanitizeDocxHtml } from './docxSanitize'

interface DocxViewerProps {
  filePath: string
}

export function DocxViewer({ filePath }: DocxViewerProps): React.JSX.Element {
  // Home M4: the room's server.
  const scope = useRoom().scope
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
        const r = await daemonCliGet<{ base64: string }>(scope, 'fs/read-binary', { path: filePath })

        if (cancelled) return

        const binary = atob(r.base64)
        const uint8 = new Uint8Array(binary.length)
        for (let i = 0; i < binary.length; i++) uint8[i] = binary.charCodeAt(i)
        const arrayBuffer = uint8.buffer

        // Convert DOCX to HTML using mammoth
        const result = await mammoth.convertToHtml({ arrayBuffer })

        if (cancelled) return

        // mammoth does not sanitize (a .docx link can be `javascript:`).
        setHtml(sanitizeDocxHtml(result.value))

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

  // Links in a document never navigate the K2 webview: http(s) opens the
  // way every other off-origin link does, mailto goes to the OS, and a
  // `#fragment` scrolls inside the document.
  const onClick = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    // The app-wide link handler (capture phase) may already have opened it.
    if (e.defaultPrevented) return
    const link = (e.target as HTMLElement | null)?.closest?.('a')
    if (!link || !e.currentTarget.contains(link)) return
    const href = link.getAttribute('href')
    if (!href) return
    e.preventDefault()
    if (href.startsWith('#')) {
      const id = href.slice(1)
      const el = Array.from(e.currentTarget.querySelectorAll('[id]')).find((n) => n.id === id)
      el?.scrollIntoView({ block: 'start' })
      return
    }
    const scheme = href.slice(0, Math.max(0, href.indexOf(':'))).toLowerCase()
    if (scheme === 'http' || scheme === 'https') {
      openOffOriginHttp(href)
    } else if (scheme === 'mailto') {
      void openUrl(href).catch((err) => {
        console.warn('[docx-viewer] failed to open', href, err)
      })
    }
  }, [])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-sm">
        Converting document...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[var(--color-bg)]">
        <span className="text-[var(--color-status-error-soft)] text-sm">Failed to load document</span>
        <span className="text-xs text-[var(--color-text-muted)] max-w-md text-center px-4">
          {error}
        </span>
      </div>
    )
  }

  return (
    <div
      className="docx-body p-6 overflow-y-auto h-full bg-[var(--color-bg)]"
      onClick={onClick}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
