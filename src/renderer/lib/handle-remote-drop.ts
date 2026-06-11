// K2 Connect remote-files Phase 3 — the single shared drop router.
//
// When the active daemon is REMOTE (K2 Connect), Tauri's native drag-drop
// hands the renderer a LOCAL path with no bytes (the file is on the
// client's disk). Dropping that local path into a terminal/file-tree is
// useless — the remote shell can't resolve it. So on remote we instead
// UPLOAD the bytes to the host and act on the returned REMOTE path.
//
// WHERE we upload is decided by WHAT was dropped on (target-driven model,
// per the PRD's "Target-driven destination model" table):
//   - terminal  → `<workspace>/.k2so/downloads/`, then inject the remote
//                 path into the PTY (so the agent can reference a file
//                 that really exists on the host).
//   - folder    → upload into that folder (user-chosen path).
//   - miss      → open the "Save to…" RemoteFolderPicker for a dest dir.
//
// This module is the ONE place v1 terminal, v2 terminal, and the
// window/file-tree all delegate to once they've decided `activeHost !==
// 'local'`. LOCAL drop behavior stays in the callers, unchanged.
//
// `resolveDropDestination` is PURE (no IO) so it can be unit-tested; the
// picker + upload + toast side-effects live in `executeRemoteDrop`.

import { uploadToRemote } from './upload-to-remote'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'
import { useToastStore } from '@/stores/toast'

/** What the drop landed on (hit-tested at the drop position). */
export type DropTarget =
  | { kind: 'terminal' }
  | { kind: 'folder'; path: string }
  | { kind: 'miss' }

/** Context the resolver needs that the pure function can't read itself. */
export interface DropContext {
  /**
   * The workspace root for a terminal drop — its `.k2so/downloads/` is the
   * upload destination. Required for the terminal case; for folder/miss
   * it's unused. Each terminal passes its own `cwd` so split-pane drops
   * land in the right workspace.
   */
  workspacePath?: string
  /**
   * The "Save to…" picker for the `miss` case. Defaults to the real
   * RemoteFolderPicker store; injectable so tests don't touch zustand. */
  openPicker?: () => Promise<string | null>
}

/**
 * Where an uploaded file should land + whether the caller injects the
 * resulting remote path into its PTY.
 *   - `destDir`  the daemon-side directory to upload into. `null` when the
 *                drop should be cancelled (a `miss` whose picker was
 *                dismissed) — resolved asynchronously by the executor.
 *   - `inject`   true only for the terminal case (paste the remote path).
 */
export interface DropDestination {
  destDir: string | null
  inject: boolean
}

/** Trailing-separator-tolerant join of `<workspace>/.k2so/downloads`. */
function downloadsDir(workspacePath: string): string {
  const base = workspacePath.replace(/[/\\]+$/, '')
  return `${base}/.k2so/downloads`
}

/**
 * PURE: map a drop target + context to an upload destination. No IO for
 * terminal/folder; the `miss` case can't resolve synchronously (it needs
 * the picker) so it returns `destDir:null` and the executor opens the
 * picker. Exposed for unit tests.
 */
export function resolveDropDestination(
  target: DropTarget,
  ctx: DropContext,
): DropDestination {
  switch (target.kind) {
    case 'terminal': {
      if (!ctx.workspacePath || ctx.workspacePath.length === 0) {
        // No workspace to anchor `.k2so/downloads/` — can't proceed.
        return { destDir: null, inject: true }
      }
      return { destDir: downloadsDir(ctx.workspacePath), inject: true }
    }
    case 'folder':
      return { destDir: target.path, inject: false }
    case 'miss':
      // Resolved by the executor via the picker (async).
      return { destDir: null, inject: false }
  }
}

/**
 * Run a remote drop end-to-end: resolve the destination, (for a miss)
 * prompt the "Save to…" picker, upload every dropped local file, and —
 * for the terminal case — feed the returned REMOTE paths through
 * `buildPayload` and return the payload for the caller to inject into its
 * PTY. Non-terminal drops return `null` (nothing to inject).
 *
 * Toasts: an "uploading…" info toast while bytes move, then success/error.
 *
 * @param localPaths the LOCAL file paths Tauri drag-drop handed us.
 * @param target     what was dropped on (hit-tested by the caller).
 * @param ctx        workspace path (terminal) + optional picker override.
 * @param buildPayload reuses the caller's existing terminal payload
 *   builder — same shell-quote + bracketed-paste-for-images logic, only
 *   fed the REMOTE paths. Required for the terminal case.
 * @returns the terminal payload to inject, or null (folder/miss/cancel).
 */
export async function executeRemoteDrop(
  localPaths: string[],
  target: DropTarget,
  ctx: DropContext,
  buildPayload?: (remotePaths: string[]) => string,
): Promise<string | null> {
  if (!localPaths || localPaths.length === 0) return null

  const dest = resolveDropDestination(target, ctx)

  // Resolve the destination directory. A `miss` (or a terminal with no
  // workspace anchor) has no synchronous destination → open the "Save
  // to…" picker; a dismissed picker cancels the whole drop.
  let destDir = dest.destDir
  if (destDir === null) {
    const open = ctx.openPicker ?? useRemoteFolderPickerStore.getState().open
    destDir = await open()
    if (!destDir) return null // user cancelled
  }

  const toast = useToastStore.getState()
  const noun = localPaths.length === 1 ? 'file' : `${localPaths.length} files`
  toast.addToast(`Uploading ${noun} to the host…`, 'info', 4000)

  const remotePaths: string[] = []
  try {
    for (const local of localPaths) {
      // Sequential: keeps memory bounded (each upload base64s a whole file)
      // and preserves drop order in the injected payload.
      const remote = await uploadToRemote(local, destDir)
      remotePaths.push(remote)
    }
  } catch (err) {
    toast.addToast(
      `Upload failed: ${err instanceof Error ? err.message : String(err)}`,
      'error',
    )
    return null
  }

  toast.addToast(
    `Uploaded ${noun} to the host`,
    'success',
    3000,
  )

  if (dest.inject && buildPayload) {
    return buildPayload(remotePaths)
  }
  return null
}
