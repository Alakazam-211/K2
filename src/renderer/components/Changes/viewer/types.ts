// Changes tab — the diff viewer (C3).
//
// Seam: .k2/prds/prd-changes-tab-v1.md "C2/C3 seam". C2 owns the tab, the
// list, the live store and the fetching; its `ChangesDiffSlot` renders
// `ChangesDiffViewer` from here with the slot's props. The types are C2's
// (type-only imports: nothing from the store is bundled into the viewer).

import type { ChangesDiffSlotProps, ChangesFile, DiffSide, DiffSides, DiffSidesState } from '../diff-slot-types'
import type { ChangesDiffMode } from '@/stores/tabs'

/** A `GET /cli/git/snapshot` `files[]` row. */
export type ChangesDiffFile = ChangesFile
export type ChangesDiffGroup = ChangesFile['group']
/** Inline = one editor, old lines shown above the new ones
 *  (`unifiedMergeView`); split = old and new side by side (`MergeView`). */
export type ChangesDiffLayout = ChangesDiffMode
/** `GET /cli/git/diff-file?sides=1` 200 body. */
export type DiffSidesResponse = DiffSides
export type { DiffSide, DiffSidesState }

/** The slot's props, plus an optional extra C2 may wire later. */
export interface ChangesDiffViewerProps extends ChangesDiffSlotProps {
  /** Optional: "Open in editor" (too-large fallback, file headers). The
   *  buttons only show when this is passed. */
  onOpenFile?: (repoRelPath: string) => void
}

/** A row's identity. A file with staged AND unstaged edits is two rows. */
export function diffRowKey(row: Pick<ChangesDiffFile, 'group' | 'path'>): string {
  return `${row.group}:${row.path}`
}

/** The `mode` the daemon reports for a row's diff (C2's `diffParamsFor`
 *  rules): staged → staged, unstaged → unstaged, base (with a ref) → base,
 *  anything else → worktree. */
export function expectedDiffMode(row: Pick<ChangesDiffFile, 'group'>, base: string | null): string {
  if (row.group === 'base' && base) return 'base'
  if (row.group === 'staged') return 'staged'
  if (row.group === 'unstaged') return 'unstaged'
  return 'worktree'
}
