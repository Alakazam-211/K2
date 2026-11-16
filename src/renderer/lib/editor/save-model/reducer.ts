// The save model for a plain file (build-editors.md E1c; the notes' §2
// Saving, §8 and v3.1): a pure reducer with explicit phases.
//
//   clean         nothing unsaved (just opened, or typed back to the file)
//   dirty         unsaved; autosave writes after a short idle ("Saving in a moment")
//   saving        a write is out
//   saved         the last write landed
//   failed        the write failed once: a bar with Retry
//   failed-twice  it failed again: "(second try)", Retry and Save a copy
//   conflict      the disk check refused the write and K2 can't merge
//                 (the file is gone, or only its hash came back): autosave
//                 stops until the person acts
//
// `dirty` is the tab's dot. The one rule: a failed write NEVER clears it.
// Only a write that landed, a reset to the file's text, or typing back to
// the file's text does.

export type SavePhase = 'clean' | 'dirty' | 'saving' | 'saved' | 'failed' | 'failed-twice' | 'conflict'

/** The 409 codes `fs/write-file` answers when `expect` doesn't match. */
export type ConflictCode = 'file_changed' | 'file_missing' | 'file_exists'

export interface SaveState {
  phase: SavePhase
  /** The tab's dot: the buffer differs from the file. */
  dirty: boolean
  /** Failed writes in a row (Retry and edits keep counting). */
  failures: number
  /** Why the last write failed (the route's error text). */
  reason: string | null
  /** Set while phase is 'conflict'. */
  conflict: ConflictCode | null
  /** Edited during a write: once it lands, still dirty. */
  editedWhileSaving: boolean
  /** Typed since the last failure: autosave tries again (else only Retry does). */
  editedSinceFailure: boolean
}

export type SaveEvent =
  /** The buffer changed; `dirty`: it differs from the file. */
  | { type: 'edit'; dirty: boolean }
  /** A write starts (autosave, ⌘S, blur, close, Retry, Save it again). */
  | { type: 'save' }
  /** The write landed; `dirty`: the buffer changed meanwhile. */
  | { type: 'saved'; dirty: boolean }
  | { type: 'failed'; reason: string }
  /** A refusal K2 can't merge (no text came back, or the file is gone). */
  | { type: 'conflict'; code: ConflictCode }
  /** A save outside K2 was merged into the buffer: write the result. */
  | { type: 'merged' }
  /** The buffer is the file's text again (Undo a restore, Use their version, Reload). */
  | { type: 'reset' }
  /** Unsaved words from before K2 quit came back. */
  | { type: 'restored' }

export const INITIAL_SAVE: SaveState = { phase: 'clean', dirty: false, failures: 0, reason: null, conflict: null, editedWhileSaving: false, editedSinceFailure: false }

const failedPhase = (failures: number): SavePhase => (failures >= 2 ? 'failed-twice' : 'failed')

export function saveReducer(s: SaveState, e: SaveEvent): SaveState {
  switch (e.type) {
    case 'edit':
      if (s.phase === 'saving') return { ...s, dirty: true, editedWhileSaving: true }
      // The file is gone or unknown: the buffer stays unsaved whatever it holds.
      if (s.phase === 'conflict') return { ...s, dirty: true }
      if (!e.dirty) return { ...INITIAL_SAVE, phase: s.phase === 'saved' ? 'saved' : 'clean' }
      if (s.phase === 'failed' || s.phase === 'failed-twice') return { ...s, dirty: true, editedSinceFailure: true }
      return { ...s, phase: 'dirty', dirty: true }
    case 'save':
      if (s.phase === 'saving' || !s.dirty) return s
      return { ...s, phase: 'saving', editedWhileSaving: false }
    case 'saved':
      return { ...INITIAL_SAVE, phase: e.dirty ? 'dirty' : 'saved', dirty: e.dirty }
    case 'failed': {
      const failures = s.failures + 1
      return { ...s, phase: failedPhase(failures), dirty: true, failures, reason: e.reason, conflict: null, editedSinceFailure: false }
    }
    case 'conflict':
      return { ...s, phase: 'conflict', dirty: true, conflict: e.code }
    case 'merged':
      return { ...s, phase: 'dirty', dirty: true, conflict: null }
    case 'reset':
      return INITIAL_SAVE
    case 'restored':
      return { ...INITIAL_SAVE, phase: 'dirty', dirty: true }
  }
}

/** May autosave write now? Never for a read-only or locked file, never
 *  during a conflict (the person decides), never twice at once. After a
 *  failure only a new edit starts it again (else Retry): no retry loop. */
export function canAutosave(s: SaveState, o: { locked?: boolean } = {}): boolean {
  if (o.locked || !s.dirty) return false
  if (s.phase === 'failed' || s.phase === 'failed-twice') return s.editedSinceFailure
  return s.phase === 'dirty'
}

export interface SaveStatus {
  word: string
  tone: 'ok' | 'pending' | 'bad'
}

const CONFLICT_WORD: Record<ConflictCode, string> = {
  file_missing: 'Not saved: the file was deleted or moved',
  file_changed: 'Not saved: the file changed on disk',
  file_exists: 'Not saved: a file with this name is there now',
}

/** The status line's save word. It never says saved while a write failed
 *  or a conflict waits. Null for a read-only file (nothing to save). */
export function saveStatus(s: SaveState, o: { locked?: boolean; merged?: boolean } = {}): SaveStatus | null {
  if (o.locked) return null
  switch (s.phase) {
    case 'saving':
      return { word: 'Saving', tone: 'pending' }
    case 'dirty':
      return { word: 'Saving in a moment', tone: 'pending' }
    case 'failed':
    case 'failed-twice':
      return { word: 'Not saved', tone: 'bad' }
    case 'conflict':
      return { word: CONFLICT_WORD[s.conflict ?? 'file_changed'], tone: 'bad' }
    default:
      return { word: o.merged ? 'Merged · Saved' : 'Saved', tone: 'ok' }
  }
}
