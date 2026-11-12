import { useState, useEffect, useRef } from 'react'
import {
  loadHostImageObjectUrl,
  revokeObjectUrl,
} from '@/lib/load-host-binary'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import { isMacTmpPath, isRemoteMacTmpError } from '@/lib/remote-mac-tmp'
import { useRoom } from '@/components/Room/RoomContext'
import { ZoomableImage } from './ZoomableImage'
import { useDocVersion } from './useDocVersion'

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
  // Home M4: the room's server (B's file in B's room).
  const scope = useRoom().scope
  const [url, setUrl] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [quiet, setQuiet] = useState(false)
  const [meta, setMeta] = useState<string>('')
  // Track the live object URL for cleanup even if state is stale, and the
  // file it shows.
  const urlRef = useRef<string | null>(null)
  const urlPathRef = useRef<string | null>(null)
  // A new version of this file from any writer: read it again.
  const version = useDocVersion(scope, filePath)

  // Unmount: drop the object URL on screen.
  useEffect(
    () => () => {
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
        urlPathRef.current = null
      }
    },
    [],
  )

  useEffect(() => {
    let cancelled = false
    const ac = new AbortController()
    // The room's connection identity (follows a switch for the primary
    // room; fixed for a pinned room).
    const startedKey = scope.connectionKey
    // A new version of the image on screen: keep it (and the zoom) until the
    // new bytes are in, then swap. Another path starts over.
    const reread = urlRef.current !== null && urlPathRef.current === filePath
    const dropUrl = (): void => {
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
        urlPathRef.current = null
      }
    }

    async function load(): Promise<void> {
      if (!reread) {
        setLoading(true)
        setError(null)
        setQuiet(false)
        setMeta('')
        // Drop previous URL before loading the next path.
        dropUrl()
        setUrl(null)
      }

      if (scope.isRemote && isMacTmpPath(filePath)) {
        if (!cancelled) {
          dropUrl()
          setUrl(null)
          setQuiet(true)
          setLoading(false)
        }
        return
      }

      try {
        const { url: objectUrl, mime, byteLength } = await loadHostImageObjectUrl(
          scope,
          filePath,
          { signal: ac.signal },
        )
        if (
          cancelled ||
          ac.signal.aborted ||
          scope.connectionKey !== startedKey
        ) {
          revokeObjectUrl(objectUrl)
          if (!cancelled) setLoading(false)
          return
        }
        dropUrl()
        urlRef.current = objectUrl
        urlPathRef.current = filePath
        setUrl(objectUrl)
        setMeta(`${mime} · ${byteLength} bytes`)
        setError(null)
        setLoading(false)
      } catch (err) {
        if (cancelled) return
        if (err instanceof Error && err.name === 'AbortError') return
        if (isHostSwitchedError(err)) {
          setLoading(false)
          return
        }
        dropUrl()
        setUrl(null)
        if (isRemoteMacTmpError(err)) {
          setQuiet(true)
          setLoading(false)
          return
        }
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
        setLoading(false)
      }
    }

    void load()

    return () => {
      cancelled = true
      ac.abort()
    }
    // `scope` is read live, as before (the room's connection key guards a
    // switch mid-read).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, version])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-xs font-mono">
        Loading image...
      </div>
    )
  }

  if (quiet) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-xs font-mono px-4 text-center">
        Not available on this server
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
    <ZoomableImage
      key={url}
      src={url}
      alt={alt ?? filePath.split('/').pop() ?? 'image'}
      onError={() => {
        setError(
          /\.heic$/i.test(filePath)
            ? 'HEIC preview not supported in this client'
            : `Browser could not decode this image${meta ? ` (${meta})` : ''}`,
        )
      }}
    />
  )
}
