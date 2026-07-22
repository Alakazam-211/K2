import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { extractArchive } from '@/lib/fs-transfer'

interface ZipListEntry {
  name: string
  size: number
  compressed_size?: number
  is_dir: boolean
}

interface ZipListResponse {
  entries: ZipListEntry[]
  truncated: boolean
}

interface ZipViewerProps {
  filePath: string
}

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '—'
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

/**
 * Archive preview for `.zip` files: list central-directory entries via
 * `GET /cli/fs/zip-list`, primary CTA reuses the existing server-side
 * extract job (`extractArchive` / `fs/extract`). Does not dump binary
 * into the code editor.
 */
export function ZipViewer({ filePath }: ZipViewerProps): React.JSX.Element {
  const [entries, setEntries] = useState<ZipListEntry[]>([])
  const [truncated, setTruncated] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [extracting, setExtracting] = useState(false)

  const loadList = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const r = await daemonCliGet<ZipListResponse>('fs/zip-list', { path: filePath })
      setEntries(Array.isArray(r.entries) ? r.entries : [])
      setTruncated(Boolean(r.truncated))
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setError(message)
      setEntries([])
      setTruncated(false)
    } finally {
      setLoading(false)
    }
  }, [filePath])

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      setLoading(true)
      setError(null)
      try {
        const r = await daemonCliGet<ZipListResponse>('fs/zip-list', { path: filePath })
        if (cancelled) return
        setEntries(Array.isArray(r.entries) ? r.entries : [])
        setTruncated(Boolean(r.truncated))
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
        setEntries([])
        setTruncated(false)
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [filePath])

  const handleExtract = useCallback(async () => {
    if (extracting) return
    setExtracting(true)
    try {
      // Reuses the Files-menu extract job (sibling folder + transfer card).
      await extractArchive(filePath)
    } finally {
      setExtracting(false)
    }
  }, [extracting, filePath])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-muted)] text-sm">
        Reading archive…
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[var(--color-bg)] px-4">
        <span className="text-[var(--color-status-error-soft)] text-sm">Failed to list archive</span>
        <span className="text-xs text-[var(--color-text-muted)] max-w-md text-center">{error}</span>
        <button
          type="button"
          className="text-xs text-[var(--color-accent)] hover:underline"
          onClick={() => void loadList()}
        >
          Retry
        </button>
      </div>
    )
  }

  return (
    <div className="flex h-full w-full flex-col bg-[var(--color-bg)]">
      <div className="flex flex-col gap-2 border-b border-[var(--color-border)] px-4 py-3 flex-shrink-0">
        <p className="text-sm text-[var(--color-text-primary)]">
          This is a ZIP archive. Contents below are listed from the central directory
          (nothing is extracted until you choose Extract).
        </p>
        <div className="flex items-center gap-3">
          <button
            type="button"
            disabled={extracting}
            onClick={() => void handleExtract()}
            className="px-3 py-1 text-xs font-medium bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            {extracting ? 'Extracting…' : 'Extract'}
          </button>
          <span className="text-[10px] text-[var(--color-text-muted)]">
            {entries.length} entr{entries.length === 1 ? 'y' : 'ies'}
            {truncated ? ' (truncated)' : ''}
          </span>
        </div>
        {truncated && (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            Showing the first {entries.length} entries. Extract still unpacks the full archive
            (subject to server limits).
          </p>
        )}
      </div>

      <div className="flex-1 overflow-y-auto">
        {entries.length === 0 ? (
          <div className="p-4 text-xs text-[var(--color-text-muted)]">Archive is empty.</div>
        ) : (
          <table className="w-full text-xs font-mono">
            <thead className="sticky top-0 bg-[var(--color-bg-stripe)] text-[var(--color-text-muted)]">
              <tr className="border-b border-[var(--color-border)] text-left">
                <th className="px-4 py-1.5 font-medium">Name</th>
                <th className="px-4 py-1.5 font-medium w-28 text-right">Size</th>
                <th className="px-4 py-1.5 font-medium w-28 text-right">Compressed</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e, i) => (
                <tr
                  key={`${e.name}-${i}`}
                  className="border-b border-[var(--color-border)]/40 text-[var(--color-text-primary)] hover:bg-[var(--color-bg-stripe)]/50"
                >
                  <td className="px-4 py-1 truncate max-w-0" title={e.name}>
                    <span className="inline-flex items-center gap-1.5 min-w-0">
                      {e.is_dir ? (
                        <span className="text-[var(--color-text-muted)] flex-shrink-0" aria-hidden>
                          📁
                        </span>
                      ) : (
                        <span className="text-[var(--color-text-muted)] flex-shrink-0" aria-hidden>
                          📄
                        </span>
                      )}
                      <span className="truncate">{e.name}</span>
                    </span>
                  </td>
                  <td className="px-4 py-1 text-right text-[var(--color-text-muted)] whitespace-nowrap">
                    {e.is_dir ? '—' : formatBytes(e.size)}
                  </td>
                  <td className="px-4 py-1 text-right text-[var(--color-text-muted)] whitespace-nowrap">
                    {e.is_dir || e.compressed_size == null
                      ? '—'
                      : formatBytes(e.compressed_size)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}
