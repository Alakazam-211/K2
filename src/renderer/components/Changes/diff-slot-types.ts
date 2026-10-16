// C2/C3 seam (prd-changes-tab-v1, "C2/C3 seam").
//
// C2 (the Changes tab: list, actions, live store) owns data and view state.
// C3 (the @codemirror/merge viewer under `Changes/viewer/*`) renders it.
// `ChangesDiffSlot` is the one place C2 mounts the viewer; C3 fills it.
//
// Rules:
//   - C2 fetches the selected file's sides (`git/diff-file?sides=1`) and
//     refetches when the snapshot `rev` moves; the viewer never fetches the
//     single-file diff itself.
//   - In `view: 'all'` the viewer loads each file lazily with `loadSides`
//     (same route, same mode rules, C2's scope). Reload sections when `rev`
//     changes.
//   - `mode` / `view` toggles live in C2's toolbar and are saved in the
//     layout; the viewer only reads them.
//   - Binary / too-large sides arrive with `text: null`; the viewer shows
//     its fallback.

import type { ChangesDiffMode, ChangesView } from '@/stores/tabs'
import type { ChangesFile, DiffSides } from '@/stores/git-changes'

export type { ChangesFile, DiffSides, DiffSide } from '@/stores/git-changes'

/** The selected file's sides, as C2 holds them. `data` stays set while a
 *  refresh is loading, so the viewer can keep painting the old text. */
export type DiffSidesState =
  | { status: 'idle' }
  | { status: 'loading'; data?: DiffSides }
  | { status: 'ready'; data: DiffSides }
  | { status: 'error'; error: string; data?: DiffSides }

export interface ChangesDiffSlotProps {
  /** Absolute workspace path the routes take (`path`). */
  workspacePath: string
  /** 'single' = `file` only; 'all' = every row of `files` in one scroll. */
  view: ChangesView
  /** Inline (unified) or split (side-by-side). */
  mode: ChangesDiffMode
  /** Compare ref; null = HEAD (staged / unstaged rows). */
  base: string | null
  /** The selected row (single view), or null. */
  file: ChangesFile | null
  /** Every row the list shows, in list order (view all). */
  files: ChangesFile[]
  /** `file`'s sides (single view). Always `{status:'idle'}` in view all. */
  sides: DiffSidesState
  /** Opaque snapshot rev; changes whenever the repo may have changed. */
  rev: string | null
  /** Fetch one row's sides (view all, lazy). */
  loadSides: (file: ChangesFile) => Promise<DiffSides>
  /** The viewer picked a file (e.g. a section header in view all). */
  onSelectFile?: (file: ChangesFile) => void
}
