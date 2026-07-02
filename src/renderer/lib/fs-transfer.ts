// 0.40.22 large-file transfers — renderer drivers for the daemon's
// compress/download fs routes.
//
// These own the WHOLE user-visible lifecycle of one operation (transfer
// card + cancel wiring + outcome toasts), mirroring `executeRemoteDrop` —
// callers like the FileTree context menu just invoke and refresh. All
// daemon IO is host-aware via `daemonCli*` (works identically on the
// local daemon and a K2 Connect remote).

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
