// 0.40.22 large-file transfers — renderer drivers for the daemon's
// compress/extract/download fs routes.
//
// These own the WHOLE user-visible lifecycle of one operation (transfer
// card + cancel wiring + outcome toasts), mirroring `executeRemoteDrop` —
// callers like the FileTree context menu just invoke and refresh. All
// daemon IO is host-aware via `daemonCli*` (works identically on the
// local daemon and a K2 Connect remote).
//
// Hosted web (VITE_WEB): no Tauri `local_download_chunk` — stream the same
// `fs/read-range` slices into a browser Blob / File System Access write and
// trigger the browser download (parity with HTML5 upload).

import { invoke } from '@tauri-apps/api/core'

import { daemonCliGet, daemonCliPost } from './daemon-cli'
import { isWebClient } from './is-web'
import { useToastStore } from '@/stores/toast'
import { useTransferProgressStore } from '@/stores/transfer-progress'

/** Wire shape of `GET /cli/fs/compress-status`. */
interface CompressStatus {
  job_id: string
  phase: 'running' | 'done' | 'failed'
  done: number
  total: number
  zip_path?: string
  error?: string
}

/** Poll cadence for the compress job. 500ms keeps the percent lively
 *  without hammering a tunneled host. */
const COMPRESS_POLL_MS = 500

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

function baseName(p: string): string {
  const seg = p.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : p
}

/**
 * Compress the folder (or file) at `path` into a sibling zip ON THE
 * ACTIVE DAEMON's disk (server-side streaming job — the tree never moves
 * over the wire). Drives a transfer card with entry-level percent +
 * Cancel, polls the job to a terminal phase, and toasts the outcome.
 *
 * @returns the final zip path on the daemon, or null (failed/cancelled —
 *   already surfaced to the user).
 */
export async function compressFolder(path: string): Promise<string | null> {
  const toast = useToastStore.getState()
  const label = baseName(path)
  const tid = useTransferProgressStore.getState().begin('compress', label)
  let cancelSent = false
  try {
    const { job_id } = await daemonCliPost<{ job_id: string }>('fs/compress', { path })
    for (;;) {
      await sleep(COMPRESS_POLL_MS)
      // Cancel is a separate POST (the job runs server-side); send it once
      // and keep polling — the worker flips the job to `failed` terminally.
      if (!cancelSent && useTransferProgressStore.getState().isCancelRequested(tid)) {
        cancelSent = true
        await daemonCliPost('fs/compress-cancel', { job_id })
      }
      const status = await daemonCliGet<CompressStatus>('fs/compress-status', { job_id })
      if (status.phase === 'running') {
        useTransferProgressStore
          .getState()
          .update(tid, status.total > 0 ? status.done / status.total : null)
        continue
      }
      if (status.phase === 'done' && status.zip_path) {
        toast.addToast(`Compressed “${label}” → ${baseName(status.zip_path)}`, 'success', 4000)
        return status.zip_path
      }
      // Terminal failure — cancellation arrives here too (the worker
      // reports "Compression cancelled").
      if (cancelSent) {
        toast.addToast('Compression cancelled', 'info', 3000)
      } else {
        toast.addToast(`Compress failed: ${status.error ?? 'unknown error'}`, 'error')
      }
      return null
    }
  } catch (err) {
    toast.addToast(
      `Compress failed: ${err instanceof Error ? err.message : String(err)}`,
      'error',
    )
    return null
  } finally {
    useTransferProgressStore.getState().end(tid)
  }
}

/** Wire shape of `GET /cli/fs/extract-status`. */
interface ExtractStatus {
  job_id: string
  phase: 'running' | 'done' | 'failed'
  done: number
  total: number
  dest_path?: string
  error?: string
}

/** Poll cadence for the extract job — same as compress. */
const EXTRACT_POLL_MS = 500

/**
 * Extract the zip at `path` into a sibling folder ON THE ACTIVE DAEMON's
 * disk (server-side job — the archive never moves over the wire). Drives
 * a transfer card with entry-level percent + Cancel, polls the job to a
 * terminal phase, and toasts the outcome.
 *
 * @returns the final dest folder path on the daemon, or null
 *   (failed/cancelled — already surfaced to the user).
 */
export async function extractArchive(path: string): Promise<string | null> {
  const toast = useToastStore.getState()
  const label = baseName(path)
  const tid = useTransferProgressStore.getState().begin('extract', label)
  let cancelSent = false
  try {
    const { job_id } = await daemonCliPost<{ job_id: string }>('fs/extract', { path })
    for (;;) {
      await sleep(EXTRACT_POLL_MS)
      if (!cancelSent && useTransferProgressStore.getState().isCancelRequested(tid)) {
        cancelSent = true
        await daemonCliPost('fs/extract-cancel', { job_id })
      }
      const status = await daemonCliGet<ExtractStatus>('fs/extract-status', { job_id })
      if (status.phase === 'running') {
        useTransferProgressStore
          .getState()
          .update(tid, status.total > 0 ? status.done / status.total : null)
        continue
      }
      if (status.phase === 'done' && status.dest_path) {
        toast.addToast(
          `Extracted “${label}” → ${baseName(status.dest_path)}`,
          'success',
          4000,
        )
        return status.dest_path
      }
      if (cancelSent) {
        toast.addToast('Extraction cancelled', 'info', 3000)
      } else {
        toast.addToast(`Extract failed: ${status.error ?? 'unknown error'}`, 'error')
      }
      return null
    }
  } catch (err) {
    toast.addToast(
      `Extract failed: ${err instanceof Error ? err.message : String(err)}`,
      'error',
    )
    return null
  } finally {
    useTransferProgressStore.getState().end(tid)
  }
}

/** Wire shape of `GET /cli/fs/read-range`. */
interface ReadRangeResponse {
  base64: string
  /** Decoded byte count of THIS slice. */
  len: number
  /** The file's total size (progress denominator). */
  size: number
  eof: boolean
}

/** Decoded bytes per ranged read — mirrors the upload chunk size; well
 *  under the daemon's 16 MB per-request cap. */
const DOWNLOAD_CHUNK_BYTES = 8 * 1024 * 1024

/** Decode a standard base64 string to bytes (browser / Node-compatible). */
export function base64ToUint8Array(b64: string): Uint8Array {
  // Prefer platform atob when present (browser / vitest happy-dom).
  if (typeof atob === 'function') {
    const bin = atob(b64)
    const out = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  }
  // Node fallback (tests without atob).
  const buf = Buffer.from(b64, 'base64')
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength)
}

/**
 * Trigger a browser download of `blob` as `filename`. Uses a temporary
 * object URL + synthetic anchor click (works in all modern browsers;
 * Chromium may also get a native save picker via {@link saveBlobInBrowser}).
 */
export function triggerBrowserDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  try {
    const a = document.createElement('a')
    a.href = url
    a.download = filename || 'download'
    a.rel = 'noopener'
    a.style.display = 'none'
    document.body.appendChild(a)
    a.click()
    a.remove()
  } finally {
    // Revoke on next tick so the browser has started the download.
    setTimeout(() => URL.revokeObjectURL(url), 1_000)
  }
}

/**
 * Persist a completed download Blob to the user's machine.
 * Prefer File System Access API when available (Chromium); fall back to
 * the classic anchor download for Safari / Firefox / denied picker.
 *
 * @returns a short human label for the toast (`filename` or `Downloads`).
 */
export async function saveBlobInBrowser(blob: Blob, filename: string): Promise<string> {
  const name = filename || 'download'
  type SavePicker = (opts?: {
    suggestedName?: string
  }) => Promise<{ createWritable: () => Promise<{ write: (d: Blob) => Promise<void>; close: () => Promise<void> }> }>
  const w = globalThis as unknown as { showSaveFilePicker?: SavePicker }
  if (typeof w.showSaveFilePicker === 'function') {
    try {
      const handle = await w.showSaveFilePicker({ suggestedName: name })
      const writable = await handle.createWritable()
      await writable.write(blob)
      await writable.close()
      return name
    } catch (err) {
      // User cancel → rethrow as cancelled so the caller can toast quietly.
      const msg = err instanceof Error ? err.message : String(err)
      if (/abort|cancel/i.test(msg) || (err instanceof DOMException && err.name === 'AbortError')) {
        throw Object.assign(new Error('Download cancelled'), { cancelled: true })
      }
      // Picker unavailable / denied → fall through to anchor download.
    }
  }
  triggerBrowserDownload(blob, name)
  return name
}

/**
 * Stream a file from the ACTIVE daemon's disk onto THIS machine.
 *
 * - **Desktop (Tauri):** lands in local `~/Downloads` via
 *   `local_download_chunk` (ordered `.part` appends + atomic rename).
 * - **Hosted web:** streams the same `fs/read-range` slices into a Blob and
 *   triggers a browser download (no Tauri FS). Cancel aborts mid-stream.
 *
 * Drives a transfer card (percent + Cancel) and toasts the outcome.
 *
 * @returns a local path (desktop) or the saved filename (web), or null.
 */
export async function downloadFile(remotePath: string): Promise<string | null> {
  if (isWebClient()) {
    return downloadFileInBrowser(remotePath)
  }
  return downloadFileDesktop(remotePath)
}

/** Desktop path: Tauri local_download_chunk → ~/Downloads. */
async function downloadFileDesktop(remotePath: string): Promise<string | null> {
  const toast = useToastStore.getState()
  const label = baseName(remotePath)
  const tid = useTransferProgressStore.getState().begin('download', label)
  const downloadId = `dl-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
  try {
    let offset = 0
    for (;;) {
      if (useTransferProgressStore.getState().isCancelRequested(tid)) {
        await invoke('local_download_abort', { downloadId })
        toast.addToast('Download cancelled', 'info', 3000)
        return null
      }
      const slice = await daemonCliGet<ReadRangeResponse>('fs/read-range', {
        path: remotePath,
        offset,
        len: DOWNLOAD_CHUNK_BYTES,
      })
      const finalPath = await invoke<string | null>('local_download_chunk', {
        downloadId,
        filename: label,
        offset,
        base64: slice.base64,
        isLast: slice.eof,
      })
      offset += slice.len
      useTransferProgressStore
        .getState()
        .update(tid, slice.size > 0 ? Math.min(offset / slice.size, 1) : 1)
      if (slice.eof) {
        if (!finalPath) {
          throw new Error('Download finalize returned no local path.')
        }
        toast.addToast(`Downloaded “${baseName(finalPath)}” to Downloads`, 'success', 4000)
        return finalPath
      }
    }
  } catch (err) {
    // Best-effort part cleanup; the real failure is what we surface.
    await invoke('local_download_abort', { downloadId }).catch(() => undefined)
    toast.addToast(
      `Download failed: ${err instanceof Error ? err.message : String(err)}`,
      'error',
    )
    return null
  } finally {
    useTransferProgressStore.getState().end(tid)
  }
}

/**
 * Hosted-web path: stream `fs/read-range` into memory, then browser save.
 * Same daemon wire as desktop; no Tauri invoke.
 */
async function downloadFileInBrowser(remotePath: string): Promise<string | null> {
  const toast = useToastStore.getState()
  const label = baseName(remotePath)
  const tid = useTransferProgressStore.getState().begin('download', label)
  const parts: Uint8Array[] = []
  try {
    let offset = 0
    for (;;) {
      if (useTransferProgressStore.getState().isCancelRequested(tid)) {
        toast.addToast('Download cancelled', 'info', 3000)
        return null
      }
      const slice = await daemonCliGet<ReadRangeResponse>('fs/read-range', {
        path: remotePath,
        offset,
        len: DOWNLOAD_CHUNK_BYTES,
      })
      if (slice.base64) {
        parts.push(base64ToUint8Array(slice.base64))
      }
      offset += slice.len
      useTransferProgressStore
        .getState()
        .update(tid, slice.size > 0 ? Math.min(offset / slice.size, 1) : 1)
      if (slice.eof) {
        // Build Blob from accumulated chunks (ArrayBuffer-backed parts).
        const blob = new Blob(parts as BlobPart[], {
          type: 'application/octet-stream',
        })
        try {
          const savedAs = await saveBlobInBrowser(blob, label)
          toast.addToast(`Downloaded “${savedAs}”`, 'success', 4000)
          return savedAs
        } catch (err) {
          if (err && typeof err === 'object' && 'cancelled' in err) {
            toast.addToast('Download cancelled', 'info', 3000)
            return null
          }
          throw err
        }
      }
    }
  } catch (err) {
    toast.addToast(
      `Download failed: ${err instanceof Error ? err.message : String(err)}`,
      'error',
    )
    return null
  } finally {
    useTransferProgressStore.getState().end(tid)
  }
}
