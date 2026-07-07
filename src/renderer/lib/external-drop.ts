// External-drop planning for the file tree (LOCAL host).
//
// INVARIANT: a drop of EXTERNAL files (from Finder/the OS) into the file
// tree is NEVER destructive — it always COPIES, regardless of modifier
// keys. The source file remains with its original owner (Finder), so a
// copy is always safe. A move here once silently relocated files off the
// user's Desktop when they believed the tree showed a remote server
// (data-loss bug, 2026-07-07). Only INTERNAL tree drags (moving an entry
// between folders within the tree) keep move semantics — that logic lives
// in FileTree's drag-out handler, not here.
//
// PURE (no IO) so it can be unit-tested; the caller performs the daemon
// call, undo push, and toast.

export interface LocalExternalDropPlan {
  /** Always the non-destructive copy endpoint — never 'fs/move'. */
  endpoint: 'fs/copy'
  payload: { sources: string[]; destination: string }
  /** Undo entry: deleting the created copies restores the pre-drop state. */
  undo: { type: 'copy'; createdPaths: string[] }
  /** Success toast, e.g. "Copied 2 items". */
  toast: string
}

/**
 * PURE: plan the daemon operation for external (OS → tree) files dropped
 * on a LOCAL-host file tree. Always resolves to a copy (see the invariant
 * above).
 *
 * @param paths        absolute source paths Tauri drag-drop handed us.
 * @param targetFolder the tree folder the drop landed on (hit-tested by
 *                     the caller; the tree root on a miss).
 */
export function planLocalExternalDrop(
  paths: string[],
  targetFolder: string,
): LocalExternalDropPlan {
  return {
    endpoint: 'fs/copy',
    payload: { sources: paths, destination: targetFolder },
    undo: {
      type: 'copy',
      createdPaths: paths.map((p) => `${targetFolder}/${p.split('/').pop()}`),
    },
    toast: `Copied ${paths.length} item${paths.length > 1 ? 's' : ''}`,
  }
}
