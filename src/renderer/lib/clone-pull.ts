// K2 Connect — "Clone to this computer" orchestration (the PULL half of
// clone-to, 0.40.22).
//
// The push flow (clone-to.ts) can't bring a workspace HOME: the source
// server only knows ITS saved connections, and it can't reach the viewer's
// machine anyway. The renderer, however, holds BOTH sides' credentials —
// the remote session it's signed in with AND the local daemon's token — so
// the pull is renderer-orchestrated:
//
//   1. [remote] `POST clone/pack` → { job_id }; poll `GET clone/pack-status`
//      to a terminal phase (async job — packing can outlive one tunneled
//      round-trip). Produces the SAME bundle format the push flow ships.
//   2. [local]  native OS folder dialog picks the destination parent (the
//      local-machine analog of the push flow's remote folder picker).
//   3. [remote→local] loop `GET fs/read-range` on the bundle and land each
//      chunk via `local_download_chunk` in `~/.k2/clone-tmp` (dest mode
//      'clone-tmp' — never the user's Downloads), driving a transfer-
//      progress card with byte-level percent + Cancel.
//   4. [remote] `POST clone/pack-cleanup` reclaims the server-side bundle
//      (best-effort — the daemon's hourly stale-prune is the backstop).
//   5. [local]  `POST clone/unpack` on the LOCAL daemon — the existing
//      import path (extract, slug-rebase, register, apply settings), which
//      also deletes the downloaded temp bundle.
//
// Every remote call goes through the injected `daemonCliGet/Post` (the
// active-host helpers), so the long-running download participates in the
// same `isPossibleAuthFailure` classification + session revival as any
// other remote request. The local calls go through `localDaemonCliPost`.
//
// Like `cloneWorkspaceTo`, all side-effecting collaborators arrive as an
// injected deps bag so the sequencing is unit-testable with mocks.

import type {
  CloneHooks,
  CloneManifestSummary,
  CloneUnpackResult,
} from './clone-to'
import { CloneCancelledError } from './clone-to'
// Static import is safe in headless tests: the store module only pulls
// zustand (no Tauri / daemon side effects at import time).
import { useTransferProgressStore } from '@/stores/transfer-progress'

/** Wire shape of `GET /cli/clone/pack-status`. */
export interface ClonePackStatus {
  job_id: string
  phase: 'running' | 'done' | 'failed'
  bundle_path?: string
  size_bytes?: number
  entry_count?: number
  scrubbed_secret_count?: number
  error?: string
}

/** Wire shape of `GET /cli/fs/read-range` (mirrors fs-transfer.ts). */
interface ReadRangeResponse {
  base64: string
  len: number
  size: number
  eof: boolean
}

/** Poll cadence for the pack job — matches fs-transfer's compress poll. */
export const PACK_POLL_MS = 500
/** Decoded bytes per ranged read — mirrors the push flow's chunk size;
 *  well under the daemon's 16 MB per-request cap. */
export const PULL_DOWNLOAD_CHUNK_BYTES = 8 * 1024 * 1024

/**
 * Side-effecting collaborators, injected so the sequencing is testable.
 * The defaults (`defaultClonePullDeps`) bind the real implementations.
 */
export interface ClonePullDeps {
  /** POST a `/cli/*` route against the ACTIVE (remote) host. */
  daemonCliPost: <T = unknown>(route: string, body?: unknown) => Promise<T>
  /** GET a `/cli/*` route against the ACTIVE (remote) host. */
  daemonCliGet: <T = unknown>(
    route: string,
    params?: Record<string, string | number | boolean | undefined | null>,
  ) => Promise<T>
  /** POST a `/cli/*` route against the LOCAL daemon (host-independent). */
  localDaemonCliPost: <T = unknown>(route: string, body?: unknown) => Promise<T>
  /** Native OS folder dialog for the LOCAL destination parent; resolves
   *  with the chosen dir, or null on cancel. */
  pickLocalFolder: () => Promise<string | null>
  /** Land one ordered download chunk in the local `~/.k2/clone-tmp`
   *  staging dir (Tauri `local_download_chunk`, dest mode 'clone-tmp').
   *  Returns the final local path on the last chunk, null otherwise. */
  localDownloadChunk: (
    downloadId: string,
    filename: string,
    offset: number,
    base64: string,
    isLast: boolean,
  ) => Promise<string | null>
  /** Remove an in-progress download's `.part` (cancel / hard failure). */
  localDownloadAbort: (downloadId: string) => Promise<void>
  /** Transfer-progress overlay hooks (byte-level percent + Cancel). */
  progress: {
    begin: (kind: 'download', label: string) => string
    update: (id: string, fraction: number | null) => void
    end: (id: string) => void
    isCancelRequested: (id: string) => boolean
  }
  /** Sleep between pack-status polls (injected so tests run instantly). */
  sleep: (ms: number) => Promise<void>
}

/** Last path segment of a path, tolerating BOTH `/` and `\` separators. */
function basename(p: string): string {
  const seg = p.split(/[/\\]/).pop()
  return seg && seg.length > 0 ? seg : p
}

/**
 * Orchestrate "Clone to this computer": pack the REMOTE workspace at
 * `projectPath` on the active host, download the bundle into the local
 * clone-tmp staging dir, and unpack + register it on the LOCAL daemon.
 *
 * PRECONDITION (enforced by the caller, not here): only invoked when the
 * active host is REMOTE — the pack/download steps target the active host.
 *
 * Returns the local unpack result on success. Throws on any failure
 * (including cancellation, as a {@link CloneCancelledError}); the error is
 * also surfaced through `hooks.onError` with the failing stage named.
 */
export async function cloneWorkspaceToThisComputer(
  projectPath: string,
  projectName: string,
  deps: ClonePullDeps,
  hooks: CloneHooks = {},
  /** See `cloneWorkspaceTo` — same `clone/bundle`-family flags, threaded
   *  into the pack job. */
  carrySecrets = true,
  includeAllHistory = true,
): Promise<CloneUnpackResult> {
  const { onStage, onBundled, onDone, onError } = hooks
  try {
    // ── 1. Pack on the REMOTE (active) host — async job ─────────────────
    onStage?.('packing')
    const { job_id } = await deps.daemonCliPost<{ job_id: string }>('clone/pack', {
      project_path: projectPath,
      carry_secrets: carrySecrets,
      live_only: !includeAllHistory,
    })
    if (!job_id) {
      throw new Error('Packing failed on the server: no job id returned.')
    }
    let packed: ClonePackStatus
    for (;;) {
      await deps.sleep(PACK_POLL_MS)
      const status = await deps.daemonCliGet<ClonePackStatus>('clone/pack-status', {
        job_id,
      })
      if (status.phase === 'running') continue
      if (status.phase === 'done' && status.bundle_path) {
        packed = status
        break
      }
      throw new Error(
        `Packing failed on the server: ${status.error ?? 'unknown error'}`,
      )
    }
    const summary: CloneManifestSummary = {
      entry_count: packed.entry_count ?? 0,
      scrubbed_secret_count: packed.scrubbed_secret_count ?? 0,
      size_bytes: packed.size_bytes ?? 0,
    }
    onBundled?.(summary)

    // Best-effort server-side bundle reclaim, shared by every exit below.
    // The daemon's hourly stale-prune is the backstop if even this fails.
    const cleanupRemote = async (): Promise<void> => {
      try {
        await deps.daemonCliPost('clone/pack-cleanup', { job_id })
      } catch (e) {
        console.warn('[clone-pull] server-side bundle cleanup failed:', e)
      }
    }

    // ── 2. Pick the LOCAL destination parent ────────────────────────────
    onStage?.('choosing-folder')
    const destParent = await deps.pickLocalFolder()
    if (destParent === null) {
      await cleanupRemote()
      throw new CloneCancelledError('Clone cancelled — no destination folder chosen.')
    }

    // ── 3. Download the bundle into local clone-tmp ─────────────────────
    onStage?.('downloading')
    const remoteBundlePath = packed.bundle_path as string
    const filename = basename(remoteBundlePath)
    const downloadId = `clone-pull-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
    const tid = deps.progress.begin(
      'download',
      `Cloning “${projectName}” to this computer…`,
    )
    let localBundlePath: string
    try {
      let offset = 0
      for (;;) {
        if (deps.progress.isCancelRequested(tid)) {
          await deps.localDownloadAbort(downloadId)
          await cleanupRemote()
          throw new CloneCancelledError('Clone cancelled.')
        }
        const slice = await deps.daemonCliGet<ReadRangeResponse>('fs/read-range', {
          path: remoteBundlePath,
          offset,
          len: PULL_DOWNLOAD_CHUNK_BYTES,
        })
        const finalPath = await deps.localDownloadChunk(
          downloadId,
          filename,
          offset,
          slice.base64,
          slice.eof,
        )
        offset += slice.len
        deps.progress.update(tid, slice.size > 0 ? Math.min(offset / slice.size, 1) : 1)
        if (slice.eof) {
          if (!finalPath) {
            throw new Error('Download finalize returned no local bundle path.')
          }
          localBundlePath = finalPath
          break
        }
      }
    } catch (err) {
      if (!(err instanceof CloneCancelledError)) {
        // Hard failure mid-stream: drop the .part and reclaim the server
        // bundle before surfacing the real error.
        await deps.localDownloadAbort(downloadId).catch(() => undefined)
        await cleanupRemote()
      }
      throw err
    } finally {
      deps.progress.end(tid)
    }

    // ── 4. Reclaim the server-side bundle (downloaded — done with it) ───
    await cleanupRemote()

    // ── 5. Unpack + register on the LOCAL daemon ────────────────────────
    //     `clone/unpack` extracts at recomputed paths (incl. the Claude
    //     session-history slug rebase), registers the workspace, applies
    //     the bundled settings, and deletes the temp bundle itself.
    onStage?.('unpacking')
    let result: CloneUnpackResult
    try {
      result = await deps.localDaemonCliPost<CloneUnpackResult>('clone/unpack', {
        bundle_path: localBundlePath,
        dest_parent: destParent,
      })
    } catch (err) {
      // Unpack never ran to the point of deleting the temp bundle — do it
      // ourselves (best-effort; the local daemon's stale-prune backstops).
      await deps
        .localDaemonCliPost('fs/delete', { paths: [localBundlePath], permanent: true })
        .catch((e) => console.warn('[clone-pull] temp bundle cleanup failed:', e))
      throw new Error(
        `Unpacking on this computer failed: ${err instanceof Error ? err.message : String(err)}`,
      )
    }
    if (!result?.dest_path) {
      throw new Error('Unpacking on this computer failed: no dest_path returned.')
    }

    onStage?.('done')
    onDone?.(result)
    return result
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : String(err)
    onStage?.('error')
    onError?.(message)
    throw err
  }
}

/**
 * Build the default (real) dep bag for {@link cloneWorkspaceToThisComputer}.
 * Bound lazily inside the function bodies so importing this module in a
 * headless test env (vitest) never pulls Tauri/store side effects until
 * called — same pattern as `defaultCloneDeps`.
 */
export function defaultClonePullDeps(): ClonePullDeps {
  return {
    daemonCliPost: async (route, body) => {
      const { daemonCliPost } = await import('./daemon-cli')
      return daemonCliPost(route, body)
    },
    daemonCliGet: async (route, params) => {
      const { daemonCliGet } = await import('./daemon-cli')
      return daemonCliGet(route, params)
    },
    localDaemonCliPost: async (route, body) => {
      const { localDaemonCliPost } = await import('./daemon-cli')
      return localDaemonCliPost(route, body)
    },
    pickLocalFolder: async () => {
      // Deliberately NOT `pickWorkspaceFolder()` — that helper is host-
      // aware and would open the REMOTE picker while a remote is active;
      // the pull's destination is always THIS machine's disk.
      const { invoke } = await import('@tauri-apps/api/core')
      return invoke<string | null>('projects_pick_folder')
    },
    localDownloadChunk: async (downloadId, filename, offset, base64, isLast) => {
      const { invoke } = await import('@tauri-apps/api/core')
      return invoke<string | null>('local_download_chunk', {
        downloadId,
        filename,
        offset,
        base64,
        isLast,
        dest: 'clone-tmp',
      })
    },
    localDownloadAbort: async (downloadId) => {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('local_download_abort', { downloadId, dest: 'clone-tmp' })
    },
    progress: {
      begin: (kind, label) => useTransferProgressStore.getState().begin(kind, label),
      update: (id, fraction) => useTransferProgressStore.getState().update(id, fraction),
      end: (id) => useTransferProgressStore.getState().end(id),
      isCancelRequested: (id) =>
        useTransferProgressStore.getState().isCancelRequested(id),
    },
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  }
}
