// 0.40.22 large-file transfers — renderer drivers for the daemon's
// compress/extract/download fs routes.
//
// These own the WHOLE user-visible lifecycle of one operation (transfer
// card + cancel wiring + outcome toasts), mirroring `executeRemoteDrop` —
// callers like the FileTree context menu just invoke and refresh. All
// daemon IO is host-aware via `daemonCli*` (works identically on the
// local daemon and a K2 Connect remote).

import { invoke } from '@tauri-apps/api/core'

import { daemonCliGet, daemonCliPost } from './daemon-cli'
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

/**
 * Stream a file from the ACTIVE daemon's disk into the local `~/Downloads`
 * (collision-safe naming), any size: loops `GET fs/read-range` (bounded
 * per-request memory on both sides) and lands each chunk via the
 * `local_download_chunk` Tauri command — ordered `.part` appends, fsync +
 * atomic rename on the last chunk. Drives a transfer card (percent +
 * Cancel; cancel aborts the part) and toasts the outcome.
 *
 * @returns the final LOCAL path, or null (failed/cancelled — surfaced).
 */
export async function downloadFile(remotePath: string): Promise<string | null> {
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
