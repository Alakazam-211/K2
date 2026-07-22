// Audio / video preview for FileViewerPane.
//
// PRODUCT RULE (R4/R5): Playback ALWAYS happens on the **viewer client**
// (this webview / browser) via HTML5 <audio>/<video>. Bytes arrive from
// the daemon over fs/read-binary (or ranged assembly) and become a Blob
// URL on THIS machine. NEVER open host media players, NEVER play through
// the server machine’s speakers/display.
//
// Intentionally does NOT import or call `convertFileSrc` — that path
// only works for local Tauri asset URLs and breaks remote + web clients.

import { useEffect, useState } from 'react'
import {
  loadHostBinary,
  bytesToObjectUrl,
  READ_BINARY_MAX_BYTES,
  HostBinaryTooLargeError,
} from '@/lib/load-host-binary'
import { mediaMimeFromPath } from './fileCategory'

export type MediaKind = 'audio' | 'video'

interface MediaViewerProps {
  filePath: string
  kind: MediaKind
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

export function MediaViewer({ filePath, kind }: MediaViewerProps): React.JSX.Element {
  const [objectUrl, setObjectUrl] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [progress, setProgress] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [sizeLabel, setSizeLabel] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    let createdUrl: string | null = null
    const ac = new AbortController()

    async function load(): Promise<void> {
      setLoading(true)
      setError(null)
      setObjectUrl(null)
      setProgress(null)
      setSizeLabel(null)

      try {
        // Prefer single-shot read-binary (≤50 MB). On size rejection,
        // assemble via read-range so typical clips still play on the
        // client. Soft UX cap: refuse to assemble multi-GB media in RAM.
        const SOFT_RANGE_CAP = 200 * 1024 * 1024 // 200 MB assembled

        const bytes = await loadHostBinary(filePath, {
          allowRangeAssembly: true,
          signal: ac.signal,
          onProgress: (ratio) => {
            if (!cancelled) setProgress(ratio)
          },
        })

        if (cancelled) return

        if (bytes.byteLength > SOFT_RANGE_CAP) {
          throw new HostBinaryTooLargeError(
            `File is ${formatBytes(bytes.byteLength)} — too large to preview in the viewer (limit ${formatBytes(SOFT_RANGE_CAP)}). Use Reveal / Download instead.`,
            bytes.byteLength,
          )
        }

        const mime = mediaMimeFromPath(filePath)
        createdUrl = bytesToObjectUrl(bytes, mime)
        setSizeLabel(formatBytes(bytes.byteLength))
        setObjectUrl(createdUrl)
        setLoading(false)
        setProgress(null)
      } catch (err) {
        if (cancelled || (err instanceof Error && err.name === 'AbortError')) return
        const message = err instanceof Error ? err.message : String(err)
        // Surface the 50 MB read-binary cap clearly when range assembly
        // is not available or also fails.
        const friendly = message.toLowerCase().includes('too large')
          ? `${message} (daemon single-read cap is ${formatBytes(READ_BINARY_MAX_BYTES)}; larger files use ranged assembly when possible.)`
          : message
        setError(friendly)
        setLoading(false)
        setProgress(null)
      }
    }

    void load()

    return () => {
      cancelled = true
      ac.abort()
      if (createdUrl) {
        URL.revokeObjectURL(createdUrl)
      }
    }
  }, [filePath])

  // Revoke when objectUrl identity changes (path change sets a new one).
  useEffect(() => {
    return () => {
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [objectUrl])

  if (loading) {
    const pct =
      progress != null && Number.isFinite(progress)
        ? ` ${Math.round(progress * 100)}%`
        : ''
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 bg-[var(--color-bg)] text-sm text-[var(--color-text-muted)]">
        <span>
          Loading {kind}…{pct}
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)]/80">
          Fetching bytes from host — playback stays on this device
        </span>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[var(--color-bg)] px-6 text-center">
        <span className="text-sm text-[var(--color-status-error-soft)]">
          Failed to load {kind}
        </span>
        <span className="max-w-md text-xs text-[var(--color-text-muted)]">{error}</span>
        <span className="max-w-sm text-[10px] text-[var(--color-text-muted)]">
          Media always plays on this device (never the server). If the file is larger than{' '}
          {formatBytes(READ_BINARY_MAX_BYTES)}, try Reveal or Download from the Files drawer.
        </span>
      </div>
    )
  }

  if (!objectUrl) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-sm text-[var(--color-text-muted)]">
        No media data
      </div>
    )
  }

  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[var(--color-bg)] p-6">
      <p className="text-[11px] text-[var(--color-text-muted)]">
        Playing on this device
        {sizeLabel ? ` · ${sizeLabel}` : ''}
      </p>
      {kind === 'audio' ? (
        <audio
          // Client-only playback — Blob URL from daemon bytes, never host AV.
          controls
          src={objectUrl}
          className="w-full max-w-xl"
          preload="metadata"
        >
          Your browser does not support audio playback.
        </audio>
      ) : (
        <video
          // Client-only playback — Blob URL from daemon bytes, never host AV.
          controls
          playsInline
          src={objectUrl}
          className="max-h-[min(70vh,720px)] w-full max-w-4xl bg-black"
          preload="metadata"
        >
          Your browser does not support video playback.
        </video>
      )}
    </div>
  )
}
