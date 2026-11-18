// The live editor's notices, in words (editors-notes.md §2 Live, §8 Copy, v3,
// v3.1 and v3.2): one bar at a time under the header, a bold headline of 70
// characters or fewer, one muted line only when needed, then the actions. The
// same stacked anatomy as the save notices (SaveNoticeBar).
//
// Kept-pair marks and their "kept both" counts are E4c (K2 Cores' line
// ranges), so the merged bar says the merge rule without counting pairs.

import type { SaveNoticeView } from '@/lib/editor/save-model/notices'
import type { ConnectionState } from '@/lib/live-view/shared/notices'
import type { LiveDocClose } from './live-doc-source'

export type LiveActionId =
  | 'retry'
  | 'reopen'
  | 'compare'
  | 'close-compare'
  | 'retry-before'
  | 'undo-theirs'
  | 'confirm-undo-theirs'
  | 'cancel'
  | 'use-theirs'
  | 'redo'
  | 'undo'
  | 'save-again'
  | 'copy-note'
  | 'open-settings'
  | 'dismiss'

export type LiveNoticeView = SaveNoticeView<LiveActionId>

export type EditorLiveNotice =
  | { kind: 'closed'; closed: LiveDocClose }
  | { kind: 'connection'; state: ConnectionState; offline: number; synced: number | null; caughtUp: number; retried: boolean }
  | { kind: 'merged'; comparing: boolean }
  | { kind: 'confirm-undo-theirs' }
  | { kind: 'undid-theirs' }
  | { kind: 'used-theirs' }
  | { kind: 'theirs-gone'; comparing: boolean }
  | { kind: 'theirs-failed'; reason: string }
  | { kind: 'before-failed' }
  | { kind: 'before-gone' }
  | { kind: 'missing' }
  | { kind: 'save-failed'; reason: string }
  | { kind: 'declined'; message: string }
  | { kind: 'note'; copied: boolean }
  | { kind: 'read-only'; owner: string | null }
  | { kind: 'live-off' }

const plural = (n: number, w: string): string => `${n} ${w}${n === 1 ? '' : 's'}`
const OFFLINE_LINE = 'Your edits are saved on this computer and sync when you’re back online.'

/** "Ask Sam for edit access", or the fallback when the welcome named no one. */
export const askAccess = (owner: string | null): string => (owner ? `Ask ${owner} for edit access` : 'Ask the file’s owner for edit access')

/** Why Write is off in a read-only live file. */
export const readOnlyReason = (owner: string | null): string => `You can view this file but not edit it. ${askAccess(owner)}`

/** The daemon's `message` on a close: on the close itself (a close after
 *  the welcome), or on the refusal's `close` (a `LiveConnectError`: a
 *  refused open, whose own Error message doesn't survive the editor copying
 *  its fields). Without it a refused open loses the daemon's reason. */
export function closeMessage(c: unknown): string | undefined {
  const o = c as { message?: unknown; close?: { message?: unknown } } | null
  const m = typeof o?.message === 'string' ? o.message : typeof o?.close?.message === 'string' ? o.close.message : undefined
  return m && m.trim() ? m : undefined
}

const LEAD = 'This file can’t be edited live: '
const sentence = (s: string): string => (/[.!?]$/.test(s) ? s : `${s}.`)

/** A live open the daemon refused (`closed {declined}`), by its code: each
 *  refusal in its own words, never "too large, or not text" for a rule. The
 *  daemon's own words where the code is broad (`protected`: the vault, git's
 *  folder or the workspace's private `.k2` folder) or one this build doesn't
 *  know; null when the close says nothing (no code, no message). */
export function declinedWords(c: { code?: string; message?: string }): string | null {
  const why = c.message?.trim() || null
  switch (c.code) {
    case 'secret':
      return `${LEAD}it looks like it holds a secret.`
    case 'generated':
      return `${LEAD}it’s made from other files. Edit its source instead.`
    case 'not_a_text_doc':
      return `${LEAD}it isn’t text.`
    case 'too_large':
      return `${LEAD}it’s too large.`
    case 'not_utf8':
      return `${LEAD}it isn’t UTF-8 text.`
    case 'binary':
      return `${LEAD}it holds binary data, not text.`
    case 'mixed_line_endings':
      return `${LEAD}its lines end in more than one way (mixed line endings).`
    case 'workspace_not_found':
      return `${LEAD}it isn’t inside a workspace on this server.`
    case 'workspace_mismatch':
    case 'path_escapes':
    case 'other_workspace':
      return `${LEAD}it’s outside this workspace.`
    case 'board_file':
      return `${LEAD}it’s a Board file.`
    default:
      return why ? `${LEAD}${sentence(why)}` : null
  }
}

/** Why a closed live editor stopped (not the core turned off: that hands over to the plain editor). */
export function closedWords(c: LiveDocClose): string {
  switch (c.code === 'docs_unavailable' ? c.code : c.reason) {
    case 'docs_unavailable':
      return c.message ?? 'Live documents are off on this server.'
    case 'idle':
      return 'This tab was idle, so live editing paused.'
    case 'kicked':
      return 'You were removed from this file.'
    case 'rights_lost':
      return 'You can no longer open this file.'
    case 'deleted':
      return 'This file’s live document was deleted.'
    case 'moved':
      return c.path ? `This file was renamed to ${c.path}. Open it there.` : 'This file was renamed. Open it at its new name.'
    case 'declined':
      return declinedWords(c) ?? 'This file can’t be edited live: too large, or not text.'
    case 'not_found':
      return 'This file is gone.'
    case 'auth':
      return 'Sign in again to edit live.'
    case 'released':
      return 'The server is restarting. Reopen in a moment.'
    default:
      return 'The server closed live editing for this file.'
  }
}

function connection(n: Extract<EditorLiveNotice, { kind: 'connection' }>): LiveNoticeView {
  const so = n.offline > 0 ? ` ${plural(n.offline, 'offline edit')} so far.` : ''
  switch (n.state) {
    case 'reconnecting':
      return { tone: 'warn', role: 'status', icon: 'offline', title: `Connection lost; K2 keeps trying.${so}`, line: OFFLINE_LINE, actions: [{ id: 'retry', label: 'Retry now' }], closable: false }
    case 'trying':
      return { tone: 'info', role: 'status', icon: 'info', busy: true, title: 'Trying to reach the server now.', line: OFFLINE_LINE, actions: [{ id: 'retry', label: 'Trying now', disabled: true, hint: 'K2 is trying now' }], closable: false }
    case 'cant':
      return {
        tone: 'bad',
        role: 'alert',
        icon: 'offline',
        title: `Can’t reach the server.${so}`,
        line: `${n.retried ? 'Tried again just now. ' : ''}Your edits are saved on this computer and sync when the server is back.`,
        actions: [{ id: 'retry', label: 'Retry', primary: true }],
        closable: false,
      }
    case 'offline':
      return { tone: 'warn', role: 'status', icon: 'offline', title: `You’re offline.${so}`, line: OFFLINE_LINE, actions: [{ id: 'retry', label: 'Retry' }], closable: false }
    case 'back': {
      const synced = n.synced ?? 0
      const came = n.caughtUp > 0 ? `${plural(n.caughtUp, 'change')} came in while you were away. ${n.caughtUp === 1 ? 'It’s' : 'They’re'} first in Changes.` : null
      return { tone: 'info', role: 'status', icon: 'ok', title: synced > 0 ? `Back online. Your ${synced === 1 ? 'offline edit' : `${synced} offline edits`} synced.` : 'Back online.', line: came, actions: [], closable: true }
    }
  }
}

const COMPARE = (comparing: boolean) => (comparing ? { id: 'close-compare' as const, label: 'Close compare' } : { id: 'compare' as const, label: 'Compare' })
const UNDO_THEIRS = { id: 'undo-theirs' as const, label: 'Undo their save', hint: 'Puts back the text from before that save. This also replaces edits made since' }

export function liveNoticeView(n: EditorLiveNotice, fileName: string): LiveNoticeView {
  switch (n.kind) {
    case 'closed':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: 'Live editing stopped.', line: closedWords(n.closed), actions: n.closed.reason === 'moved' || n.closed.reason === 'not_found' || n.closed.reason === 'deleted' ? [] : [{ id: 'reopen', label: 'Reopen', primary: true }], closable: false }
    case 'connection':
      return connection(n)
    case 'merged':
      return {
        tone: 'info',
        role: 'status',
        icon: 'info',
        title: 'Saved outside K2. Its changes are merged in.',
        line: 'Where you both changed the same lines, K2 kept both versions for you to tidy.',
        actions: [COMPARE(n.comparing), UNDO_THEIRS, { id: 'use-theirs', label: 'Use their version', hint: 'Takes the file as it was saved outside K2. Your words stay in History, and Undo brings them back' }],
        closable: true,
      }
    case 'confirm-undo-theirs':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: 'Undo their save? This also replaces edits made since.', line: 'Their save stays in History.', actions: [{ id: 'confirm-undo-theirs', label: 'Undo their save', primary: true }, { id: 'cancel', label: 'Cancel' }], closable: false }
    case 'undid-theirs':
      return { tone: 'info', role: 'status', icon: 'history', title: 'Put back the text from before that save.', line: 'Their save stays in History.', actions: [{ id: 'redo', label: 'Redo' }], closable: true }
    case 'used-theirs':
      return { tone: 'info', role: 'status', icon: 'history', title: 'Using the version saved outside K2.', line: 'Your words are in History. Undo brings them back.', actions: [{ id: 'undo', label: 'Undo' }], closable: true }
    case 'theirs-gone':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: 'K2 no longer has the text of that save.', line: 'A newer save came, or K2 restarted. Compare, or undo their save.', actions: [COMPARE(n.comparing), UNDO_THEIRS], closable: true }
    case 'before-failed':
      return { tone: 'bad', role: 'alert', icon: 'bad', title: 'K2 couldn’t load the text from before that save.', line: 'The merged text is still here. Try again in a moment.', actions: [{ id: 'retry-before', label: 'Retry', primary: true }], closable: true }
    case 'before-gone':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: 'K2 no longer has the text from before that save.', line: 'The merged text is still here, and History may have that version.', actions: [], closable: true }
    case 'theirs-failed':
      return { tone: 'bad', role: 'alert', icon: 'bad', title: `Couldn’t use their version: ${n.reason}.`, line: 'The merged text is still here.', actions: [{ id: 'use-theirs', label: 'Retry', primary: true }], closable: true }
    case 'missing':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: `${fileName} was deleted or moved outside K2.`, line: 'Your text is still here. Save it again to keep working on it.', actions: [{ id: 'save-again', label: 'Save it again', primary: true }], closable: false }
    case 'save-failed':
      return { tone: 'bad', role: 'alert', icon: 'bad', title: `Couldn’t save ${fileName}: ${n.reason}.`, line: 'Your text is still here.', actions: [{ id: 'save-again', label: 'Retry', primary: true }], closable: false }
    case 'declined':
      return {
        tone: 'bad',
        role: 'alert',
        icon: 'warn',
        title: 'Saved outside K2 as something K2 can’t edit live.',
        line: `${n.message.trim() ? `${n.message.trim().replace(/[.\s]+$/, '')}. ` : ''}Live edits are paused. This is the last good version.`,
        actions: [{ id: 'save-again', label: 'Restore this version', primary: true, hint: `Saves this text as ${fileName} again` }],
        closable: false,
      }
    case 'note':
      return { tone: 'warn', role: 'alert', icon: 'warn', title: 'A save outside K2 removed some of your text.', line: 'It’s in History too. The rest of the save is merged in.', actions: [{ id: 'copy-note', label: n.copied ? 'Copied' : 'Copy my text' }], closable: true }
    case 'read-only':
      return { tone: 'info', role: 'status', icon: 'info', title: `Read only: you can view ${fileName} but not edit it.`, line: `${askAccess(n.owner)}.`, actions: [], closable: true }
    case 'live-off':
      return { tone: 'info', role: 'status', icon: 'info', title: 'Live editing is off for this file (Settings → Multiplayer).', line: 'It saves on its own, as before.', actions: [{ id: 'open-settings', label: 'Open Settings' }], closable: true }
  }
}
