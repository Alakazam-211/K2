// Changes tab — the diff viewer (C3).
//
// Seam: .k2/prds/prd-changes-tab-v1.md "C2/C3 seam". C2 owns the tab, the
// list, the live store and the fetching; it mounts `ChangesDiffSlot`
// (`Changes/ChangesDiffSlot.tsx`) whose body renders `ChangesDiffViewer`
// from here with the same props. The shapes below mirror C2's
// `ChangesDiffSlotProps` / `ChangesFile` / `DiffSides` structurally (C2's
// types are assignable to these), so the viewer has no import on C2's store.

/** A `GET /cli/git/snapshot` `files[]` row (C2's `ChangesFile`). */
export interface ChangesDiffFile {
  /** Repo-relative path. Ends with `/` for a collapsed untracked folder. */
  path: string
  /** The pre-rename path (status `R`). */
  oldPath: string | null
  group: ChangesDiffGroup
  /** M modified, A added, D deleted, R renamed, T typechange, U untracked, C conflicted. */
  status: string
  additions: number | null
  deletions: number | null
  binary: boolean
  dir: boolean
}

export type ChangesDiffGroup = 'conflicted' | 'staged' | 'unstaged' | 'untracked' | 'base'

/** Inline = one editor, old lines shown above the new ones
 *  (`unifiedMergeView`); split = old and new side by side (`MergeView`). */
export type ChangesDiffLayout = 'inline' | 'split'

/** One side of `diff-file?sides=1` (C2's `DiffSide`). */
export interface DiffSide {
  label: string
  exists: boolean
  text: string | null
  size: number
  binary: boolean
  tooLarge: boolean
}

/** `GET /cli/git/diff-file?sides=1` 200 body (C2's `DiffSides`). */
export interface DiffSidesResponse {
  path: string
  oldPath: string | null
  mode: string
  baseRef: string | null
  rev: string
  old: DiffSide
  new: DiffSide
  binary: boolean
  tooLarge: boolean
  additions: number | null
  deletions: number | null
}

/** The selected file's sides as C2 holds them. `data` stays set while a
 *  refresh loads, so the viewer keeps painting the old text. */
export type DiffSidesState =
  | { status: 'idle' }
  | { status: 'loading'; data?: DiffSidesResponse }
  | { status: 'ready'; data: DiffSidesResponse }
  | { status: 'error'; error: string; data?: DiffSidesResponse }

/** C2's `ChangesDiffSlotProps`, plus optional extras C2 may wire later. */
export interface ChangesDiffViewerProps {
  /** Absolute workspace path the routes take (`path`). */
  workspacePath: string
  /** `single` = `file` only; `all` = every row of `files` in one scroll. */
  view: 'single' | 'all'
  /** Inline or split. The toggle lives in C2's toolbar. */
  mode: ChangesDiffLayout
  /** Compare ref; null = HEAD. */
  base: string | null
  /** The selected row (single view), or null. */
  file: ChangesDiffFile | null
  /** Every row the list shows, in list order (view all). */
  files: ChangesDiffFile[]
  /** `file`'s sides (single view). Idle in view all. */
  sides: DiffSidesState
  /** Opaque snapshot rev; a new value reloads the loaded view-all sections. */
  rev: string | null
  /** Fetch one row's sides (view all, lazy). */
  loadSides: (file: ChangesDiffFile) => Promise<DiffSidesResponse>
  /** The viewer picked a file (a section header in view all). */
  onSelectFile?: (file: ChangesDiffFile) => void
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
