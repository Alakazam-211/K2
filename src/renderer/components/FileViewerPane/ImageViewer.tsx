import { useState, useEffect, useRef } from 'react'
import {
  loadHostImageObjectUrl,
  revokeObjectUrl,
} from '@/lib/load-host-binary'

interface ImageViewerProps {
  filePath: string
  alt?: string
}

/**
 * Host-aware image preview. Loads bytes via `fs/read-binary` → sniffs
 * real format (PNG-in-ICO, misnamed files) → Blob URL.
 * Never uses `convertFileSrc` (breaks remote + web).
 * Revokes the object URL on unmount / path change.
 */
export function ImageViewer({ filePath, alt }: ImageViewerProps): React.JSX.Element {
  const [url, setUrl] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [meta, setMeta] = useState<string>('')
  // Track the live object URL for cleanup even if state is stale.
  const urlRef = useRef<string | null>(null)

  useEffect(() => {
    let cancelled = false

    async function load(): Promise<void> {
      setLoading(true)
      setError(null)
      setMeta('')
      // Drop previous URL before loading the next path.
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
      }
      setUrl(null)

      try {
        const { url: objectUrl, mime, byteLength } = await loadHostImageObjectUrl(
          filePath,
        )
        if (cancelled) {
          revokeObjectUrl(objectUrl)
          return
        }
        urlRef.current = objectUrl
        setUrl(objectUrl)
        setMeta(`${mime} · ${byteLength} bytes`)
        setLoading(false)
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
        setLoading(false)
      }
    }

    void load()

    return () => {
      cancelled = true
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
      }
    }
  }, [filePath])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-xs font-mono">
        Loading image...
      </div>
    )
  }

  if (error) {
    const isHeic = /\.heic$/i.test(filePath) || /\.heif$/i.test(filePath)
    const isIco = /\.ico$/i.test(filePath)
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 bg-[var(--color-bg)] px-4">
        <span className="text-[var(--color-status-error-soft)] text-xs font-mono">
          Failed to load image
        </span>
        <span className="text-[var(--color-text-muted)] text-[10px] font-mono max-w-[400px] text-center">
          {isHeic
            ? 'HEIC preview may not be supported in this client. Reveal the file in the file manager instead.'
            : isIco
              ? `${error} — multi-resolution .ico files sometimes cannot be painted; try a .png export.`
              : error}
        </span>
      </div>
    )
  }

  if (!url) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-xs font-mono">
        No image data
      </div>
    )
  }

  return (
    <div className="flex flex-col items-center justify-center p-4 min-h-full bg-[var(--color-bg)] gap-2">
      <img
        src={url}
        alt={alt ?? filePath.split('/').pop() ?? 'image'}
        style={{ maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' }}
        onError={() => {
          setError(
            /\.heic$/i.test(filePath)
              ? 'HEIC preview not supported in this client'
              : `Browser could not decode this image${meta ? ` (${meta})` : ''}`,
          )
        }}
      />
    </div>
  )
}
