// The editor's save notices, in words (the notes' §2 Saving, §8 Copy, v3,
// v3.1 and v3.2): a bold headline of 70 characters or fewer, then one muted
// line, then the actions. The status line agrees with each (reducer.ts
// `saveStatus`).

import { failureHint } from './write'

export type SaveNotice =
  | { kind: 'failed'; reason: string; second: boolean }
  | { kind: 'restored' }
  | { kind: 'merged'; kept: number; comparing: boolean }
  | { kind: 'undone' }
  | { kind: 'used-theirs' }
  | { kind: 'missing' }
  /** Changed on disk, but only its hash came back (an App guest), so K2 can't merge. */
  | { kind: 'unmergeable' }
  | { kind: 'copied'; name: string }
  | { kind: 'copy-failed'; reason: string }

export type SaveActionId = 'retry' | 'save-copy' | 'undo-restore' | 'compare' | 'close-compare' | 'undo-theirs' | 'use-theirs' | 'redo-merge' | 'undo-use-theirs' | 'save-again' | 'reload' | 'dismiss'

export interface SaveAction<A extends string = SaveActionId> {
  id: A
  label: string
  primary?: boolean
  /** The tooltip, when the label needs one. */
  hint?: string
  /** Off for now (still focusable; `hint` says why). */
  disabled?: boolean
}

export interface SaveNoticeView<A extends string = SaveActionId> {
  tone: 'info' | 'warn' | 'bad'
  role: 'status' | 'alert'
  /** Which icon: a failure, a restore (history) or a note. */
  icon: 'bad' | 'history' | 'info' | 'warn' | 'ok' | 'offline'
  title: string
  line: string | null
  actions: SaveAction<A>[]
  /** An × at the end (a notice you can put away). */
  closable: boolean
  /** A spinner instead of the icon (trying to reconnect). */
  busy?: boolean
}

const RETRY: SaveAction = { id: 'retry', label: 'Retry', primary: true }
const COPY: SaveAction = { id: 'save-copy', label: 'Save a copy' }

export function saveNoticeView(n: SaveNotice, fileName: string): SaveNoticeView {
  switch (n.kind) {
    case 'failed':
      return n.second
        ? { tone: 'bad', role: 'alert', icon: 'bad', title: `Still can’t save ${fileName} (second try): ${n.reason}.`, line: failureHint(n.reason), actions: [RETRY, COPY], closable: false }
        : { tone: 'bad', role: 'alert', icon: 'bad', title: `Couldn’t save ${fileName}: ${n.reason}.`, line: 'Your words are still here.', actions: [RETRY], closable: false }
    case 'restored':
      return {
        tone: 'info',
        role: 'status',
        icon: 'history',
        title: 'Restored your unsaved changes from before K2 quit.',
        line: 'They save in a moment.',
        actions: [{ id: 'undo-restore', label: 'Undo', hint: `Back to ${fileName} as it is on disk` }],
        closable: true,
      }
    case 'merged':
      return {
        tone: 'info',
        role: 'status',
        icon: 'info',
        title: 'Saved outside K2. Its changes are merged in.',
        line: n.kept > 0 ? 'Where you both changed the same lines, K2 kept both versions for you to tidy.' : null,
        actions: [
          n.comparing ? { id: 'close-compare', label: 'Close compare' } : { id: 'compare', label: 'Compare' },
          { id: 'undo-theirs', label: 'Undo their save', hint: 'Puts back your text from before that save. This also replaces edits made since' },
          { id: 'use-theirs', label: 'Use their version', hint: 'Takes the file as it was saved outside K2. Undo brings your words back' },
        ],
        closable: true,
      }
    case 'undone':
      return { tone: 'info', role: 'status', icon: 'history', title: 'Put back your text from before that save.', line: 'It replaces their save in a moment.', actions: [{ id: 'redo-merge', label: 'Redo' }], closable: true }
    case 'used-theirs':
      return { tone: 'info', role: 'status', icon: 'history', title: 'Using the version saved outside K2.', line: 'Your words are gone from the file. Undo brings them back.', actions: [{ id: 'undo-use-theirs', label: 'Undo' }], closable: true }
    case 'missing':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: `${fileName} was deleted or moved.`, line: 'Your words are still here.', actions: [{ id: 'save-again', label: 'Save it again', primary: true }, COPY], closable: false }
    case 'unmergeable':
      return {
        tone: 'warn',
        role: 'alert',
        icon: 'warn',
        title: `${fileName} changed on disk while you had unsaved words.`,
        line: 'K2 can’t see its text to merge. Reload takes the file; Save a copy keeps yours.',
        actions: [{ id: 'reload', label: 'Reload' }, { ...COPY, primary: true }],
        closable: false,
      }
    case 'copied':
      return { tone: 'info', role: 'status', icon: 'ok', title: `Saved your words as ${n.name}.`, line: `${fileName} itself isn’t saved yet.`, actions: [], closable: true }
    case 'copy-failed':
      return { tone: 'bad', role: 'alert', icon: 'bad', title: `Couldn’t save a copy either: ${n.reason}.`, line: 'Your words are still here. Copy them out to keep them safe.', actions: [RETRY], closable: false }
  }
}

/** "README (copy).md", then "README (copy 2).md" … beside the file. */
export function copyPath(path: string, n: number): string {
  const slash = path.lastIndexOf('/')
  const dir = path.slice(0, slash + 1)
  const name = path.slice(slash + 1)
  const dot = name.lastIndexOf('.')
  const stem = dot > 0 ? name.slice(0, dot) : name
  const ext = dot > 0 ? name.slice(dot) : ''
  return `${dir}${stem} (copy${n > 1 ? ` ${n}` : ''})${ext}`
}
