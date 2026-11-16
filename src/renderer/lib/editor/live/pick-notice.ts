// Which one notice a live file shows, and what its status line says
// (build-editors.md E4a). Pure, so the order and the words are tested without
// a browser. One bar at a time:
//   1. the editor closed for good (not the core turned off: that hands over);
//   2. the connection (reconnecting, trying, can't, offline, back);
//   3. the last outside save and what you did about it (merged → Compare,
//      Undo their save with its confirm, Use their version; missing; paused);
//   4. a removed-text note;
//   5. read only, with who can grant access.

import type { SaveStatus } from '@/lib/editor/save-model/reducer'
import type { LiveDocSnapshot } from './live-doc-source'
import type { EditorLiveNotice } from './live-notices'

/** Where you are with the last outside save (keyed by its `at_ms`). */
export interface SaveFlow {
  at: number
  /** `load-failed`: the earlier text (before_version) didn't load; `before-gone`: there is none to put back. */
  step: 'merged' | 'confirm' | 'undid' | 'used' | 'gone' | 'failed' | 'load-failed' | 'before-gone' | 'dismissed'
  reason?: string
}

export interface LiveUi {
  flow: SaveFlow | null
  comparing: boolean
  backDismissed: boolean
  noteDismissed: boolean
  noteCopied: boolean
  readOnlyDismissed: boolean
}

export const LIVE_UI: LiveUi = { flow: null, comparing: false, backDismissed: false, noteDismissed: false, noteCopied: false, readOnlyDismissed: false }

export function pickLiveNotice(s: LiveDocSnapshot, ui: LiveUi): EditorLiveNotice | null {
  if (s.closed && s.closed.code !== 'core_off') return { kind: 'closed', closed: s.closed }
  if (s.connection && !(s.connection === 'back' && ui.backDismissed)) {
    return { kind: 'connection', state: s.connection, offline: s.offlineEdits, synced: s.synced, caughtUp: s.caughtUp ?? 0, retried: !!s.retried }
  }
  const os = s.outsideSave
  const f = os && ui.flow?.at === os.at_ms ? ui.flow : null
  if (os && f?.step !== 'dismissed') {
    if (os.outcome === 'ingested') {
      const step = f?.step ?? 'merged'
      if (step === 'merged') return { kind: 'merged', comparing: ui.comparing }
      if (step === 'confirm') return { kind: 'confirm-undo-theirs' }
      if (step === 'undid') return { kind: 'undid-theirs' }
      if (step === 'used') return { kind: 'used-theirs' }
      if (step === 'gone') return { kind: 'theirs-gone', comparing: ui.comparing }
      if (step === 'load-failed') return { kind: 'before-failed' }
      if (step === 'before-gone') return { kind: 'before-gone' }
      return { kind: 'theirs-failed', reason: f?.reason ?? 'the server refused it' }
    }
    if (os.outcome === 'missing' || os.outcome === 'declined') {
      if (f?.step === 'failed') return { kind: 'save-failed', reason: f.reason ?? 'the server refused it' }
      return os.outcome === 'missing' ? { kind: 'missing' } : { kind: 'declined', message: os.message }
    }
  }
  if (s.note && !ui.noteDismissed) return { kind: 'note', copied: ui.noteCopied }
  if (s.rights === 'read' && !ui.readOnlyDismissed) return { kind: 'read-only', owner: s.accessOwner }
  return null
}

/** The merge is on screen (its bar, the confirm, or "gone"): the status line says "Merged". */
export function showsMerged(s: LiveDocSnapshot, ui: LiveUi): boolean {
  const os = s.outsideSave
  if (os?.outcome !== 'ingested') return false
  const step = ui.flow?.at === os.at_ms ? ui.flow.step : 'merged'
  return step === 'merged' || step === 'confirm' || step === 'gone' || step === 'failed' || step === 'load-failed' || step === 'before-gone'
}

/** The status line's save word for a live file (none when you can't edit it). */
export function liveSaveStatus(s: LiveDocSnapshot, merged: boolean): SaveStatus | null {
  if (s.rights !== null && s.rights !== 'edit') return null
  if (s.outsideSave?.outcome === 'declined') return { word: 'Not saved: live edits are paused', tone: 'bad' }
  if (s.outsideSave?.outcome === 'missing') return { word: 'Not saved: the file was deleted or moved', tone: 'bad' }
  if (s.closed) return { word: 'Live editing stopped', tone: 'pending' }
  if (s.connection && s.connection !== 'back') return { word: 'Saved on this computer', tone: 'offline' }
  if (s.tier === 'loading') return { word: 'Opening', tone: 'pending' }
  if (s.pending > 0) return { word: 'Saving', tone: 'pending' }
  return { word: merged ? 'Merged · All changes saved' : 'All changes saved', tone: 'ok' }
}

/** Why Write is off in this live file, or null. */
export function liveReadOnly(s: LiveDocSnapshot, readOnlyReason: (owner: string | null) => string): string | null {
  if (s.rights === 'read' || s.rights === 'none') return readOnlyReason(s.accessOwner)
  if (s.outsideSave?.outcome === 'declined') return 'Live edits are paused until the file is text again'
  return null
}
