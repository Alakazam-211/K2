// The live file's one notice, its words and its status line (build-editors.md
// E4a; editors-notes.md §8–§11): connection first, then the outside save and
// what you did about it, then a removed-text note, then read only. Headlines
// stay at 70 characters or fewer; the status line never says saved while the
// file is missing or paused.

import { describe, expect, it } from 'vitest'
import type { LiveDocSnapshot } from './live-doc-source'
import { askAccess, liveNoticeView, readOnlyReason, type EditorLiveNotice } from './live-notices'
import { LIVE_UI, liveReadOnly, liveSaveStatus, pickLiveNotice, showsMerged } from './pick-notice'

const base: LiveDocSnapshot = {
  tier: 'live',
  connection: null,
  caughtUp: 0,
  retried: false,
  self: 'person:owner',
  people: [],
  agent: null,
  agentDone: false,
  activity: null,
  changes: [],
  messages: [],
  outsideSave: null,
  refusal: null,
  features: { viewAddress: true, timePosition: false, say: false },
  rights: 'edit',
  accessOwner: null,
  offlineEdits: 0,
  synced: null,
  liveEdits: [],
  announce: null,
  note: null,
  closed: null,
  beforeExternal: null,
  undoOff: null,
  othersHere: 1,
  pending: 0,
}
const snap = (p: Partial<LiveDocSnapshot>): LiveDocSnapshot => ({ ...base, ...p })
const merged = snap({ outsideSave: { outcome: 'ingested', rev: 4, at_ms: 10 }, beforeExternal: 'before' })

describe('which notice', () => {
  it('nothing while live and quiet', () => {
    expect(pickLiveNotice(base, LIVE_UI)).toBeNull()
  })

  it('the connection outranks a merged save; Back can be put away', () => {
    expect(pickLiveNotice({ ...merged, connection: 'reconnecting', offlineEdits: 2 }, LIVE_UI)).toEqual({ kind: 'connection', state: 'reconnecting', offline: 2, synced: null, caughtUp: 0, retried: false })
    expect(pickLiveNotice({ ...base, connection: 'back', synced: 2 }, { ...LIVE_UI, backDismissed: true })).toBeNull()
  })

  it('the merged save walks through confirm, undid, used and gone, keyed to that save', () => {
    expect(pickLiveNotice(merged, LIVE_UI)).toEqual({ kind: 'merged', comparing: false })
    const at = (step: 'confirm' | 'undid' | 'used' | 'gone' | 'dismissed') => pickLiveNotice(merged, { ...LIVE_UI, flow: { at: 10, step } })
    expect(at('confirm')).toEqual({ kind: 'confirm-undo-theirs' })
    expect(at('undid')).toEqual({ kind: 'undid-theirs' })
    expect(at('used')).toEqual({ kind: 'used-theirs' })
    expect(at('gone')).toEqual({ kind: 'theirs-gone', comparing: false })
    expect(at('dismissed')).toBeNull()
    // A newer save starts over.
    expect(pickLiveNotice({ ...merged, outsideSave: { outcome: 'ingested', rev: 5, at_ms: 11 } }, { ...LIVE_UI, flow: { at: 10, step: 'dismissed' } })).toEqual({ kind: 'merged', comparing: false })
  })

  it('missing, declined, the note, then read only', () => {
    expect(pickLiveNotice(snap({ outsideSave: { outcome: 'missing', at_ms: 1 } }), LIVE_UI)).toEqual({ kind: 'missing' })
    expect(pickLiveNotice(snap({ outsideSave: { outcome: 'declined', code: 'x', message: 'Not UTF-8', at_ms: 1 } }), LIVE_UI)).toEqual({ kind: 'declined', message: 'Not UTF-8' })
    expect(pickLiveNotice(snap({ rights: 'read', accessOwner: 'Sam' }), LIVE_UI)).toEqual({ kind: 'read-only', owner: 'Sam' })
    expect(pickLiveNotice(snap({ rights: 'read' }), { ...LIVE_UI, readOnlyDismissed: true })).toBeNull()
  })

  it('a close outranks everything, but the core turned off hands over without a bar', () => {
    expect(pickLiveNotice(snap({ closed: { reason: 'idle' }, connection: 'reconnecting' }), LIVE_UI)?.kind).toBe('closed')
    expect(pickLiveNotice(snap({ closed: { reason: 'closed', code: 'core_off' } }), LIVE_UI)).toBeNull()
  })
})

describe('the words', () => {
  const v = (n: EditorLiveNotice) => liveNoticeView(n, 'README.md')
  it('connection states (n01–n05), with the same offline count', () => {
    const c = (state: 'reconnecting' | 'trying' | 'cant' | 'offline' | 'back', o: Partial<Extract<EditorLiveNotice, { kind: 'connection' }>> = {}) => v({ kind: 'connection', state, offline: 2, synced: null, caughtUp: 0, retried: false, ...o })
    expect([c('reconnecting').title, c('reconnecting').line, c('reconnecting').actions.map((a) => a.label)]).toEqual(['Connection lost; K2 keeps trying. 2 offline edits so far.', 'Your edits are saved on this computer and sync when you’re back online.', ['Retry now']])
    expect(c('trying').busy).toBe(true)
    expect(c('trying').actions).toEqual([{ id: 'retry', label: 'Trying now', disabled: true, hint: 'K2 is trying now' }])
    expect(c('cant', { retried: true }).title).toBe('Can’t reach the server. 2 offline edits so far.')
    expect(c('cant', { retried: true }).line).toMatch(/^Tried again just now\./)
    expect(c('offline').title).toBe('You’re offline. 2 offline edits so far.')
    expect(c('back', { synced: 2 }).title).toBe('Back online. Your 2 offline edits synced.')
    expect(c('back', { synced: 1, caughtUp: 1 }).title).toBe('Back online. Your offline edit synced.')
    expect(c('back', { synced: 1, caughtUp: 1 }).line).toBe('1 change came in while you were away. It’s first in Changes.')
  })

  it('the merged save and its steps', () => {
    const m = v({ kind: 'merged', comparing: false })
    expect(m.title).toBe('Saved outside K2. Its changes are merged in.')
    expect(m.actions.map((a) => a.label)).toEqual(['Compare', 'Undo their save', 'Use their version'])
    expect(m.actions[1].hint).toMatch(/This also replaces edits made since/)
    expect(m.closable).toBe(true)
    expect(v({ kind: 'merged', comparing: true }).actions[0].label).toBe('Close compare')
    expect(v({ kind: 'confirm-undo-theirs' }).title).toBe('Undo their save? This also replaces edits made since.')
    expect(v({ kind: 'used-theirs' }).actions.map((a) => a.label)).toEqual(['Undo'])
    expect(v({ kind: 'missing' }).actions.map((a) => a.label)).toEqual(['Save it again'])
    expect(v({ kind: 'declined', message: 'It is not UTF-8.' }).line).toBe('It is not UTF-8. Live edits are paused. This is the last good version.')
    expect(v({ kind: 'live-off' }).title).toBe('Live editing is off for this file (Settings → Multiplayer).')
  })

  it('every headline is 70 characters or fewer', () => {
    const all: EditorLiveNotice[] = [
      { kind: 'connection', state: 'reconnecting', offline: 12, synced: null, caughtUp: 0, retried: false },
      { kind: 'connection', state: 'cant', offline: 12, synced: null, caughtUp: 0, retried: true },
      { kind: 'connection', state: 'back', offline: 0, synced: 12, caughtUp: 3, retried: false },
      { kind: 'merged', comparing: false },
      { kind: 'confirm-undo-theirs' },
      { kind: 'undid-theirs' },
      { kind: 'used-theirs' },
      { kind: 'theirs-gone', comparing: false },
      { kind: 'missing' },
      { kind: 'declined', message: '' },
      { kind: 'note', copied: false },
      { kind: 'read-only', owner: 'Sam' },
      { kind: 'live-off' },
      { kind: 'closed', closed: { reason: 'idle' } },
    ]
    for (const n of all) expect(v(n).title.length, v(n).title).toBeLessThanOrEqual(70)
  })

  it('read only names who can grant access, or the file’s owner', () => {
    expect(askAccess('Sam')).toBe('Ask Sam for edit access')
    expect(askAccess(null)).toBe('Ask the file’s owner for edit access')
    expect(liveReadOnly(snap({ rights: 'read', accessOwner: 'Sam' }), readOnlyReason)).toBe('You can view this file but not edit it. Ask Sam for edit access')
    expect(liveReadOnly(snap({ outsideSave: { outcome: 'declined', code: '', message: '', at_ms: 1 } }), readOnlyReason)).toBe('Live edits are paused until the file is text again')
    expect(liveReadOnly(base, readOnlyReason)).toBeNull()
  })
})

describe('the status line', () => {
  it('all changes saved while live, Saving while yours are out, Merged after a merge', () => {
    expect(liveSaveStatus(base, false)).toEqual({ word: 'All changes saved', tone: 'ok' })
    expect(liveSaveStatus(snap({ pending: 1 }), false)).toEqual({ word: 'Saving', tone: 'pending' })
    expect(showsMerged(merged, LIVE_UI)).toBe(true)
    expect(liveSaveStatus(merged, showsMerged(merged, LIVE_UI))).toEqual({ word: 'Merged · All changes saved', tone: 'ok' })
    expect(showsMerged(merged, { ...LIVE_UI, flow: { at: 10, step: 'used' } })).toBe(false)
  })

  it('disconnected: saved on this computer; missing and paused never say saved; read only says nothing', () => {
    expect(liveSaveStatus(snap({ connection: 'reconnecting' }), false)).toEqual({ word: 'Saved on this computer', tone: 'offline' })
    expect(liveSaveStatus(snap({ connection: 'back' }), false)?.word).toBe('All changes saved')
    expect(liveSaveStatus(snap({ outsideSave: { outcome: 'missing', at_ms: 1 } }), false)).toEqual({ word: 'Not saved: the file was deleted or moved', tone: 'bad' })
    expect(liveSaveStatus(snap({ outsideSave: { outcome: 'declined', code: '', message: '', at_ms: 1 } }), false)).toEqual({ word: 'Not saved: live edits are paused', tone: 'bad' })
    expect(liveSaveStatus(snap({ rights: 'read' }), false)).toBeNull()
  })
})
