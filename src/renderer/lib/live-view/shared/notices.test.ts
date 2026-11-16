// The notice bar's words: the kit's four outside-save outcomes, a refusal,
// and the connection states, each with one next step; one notice at a time,
// connection first. And the header's one state word.

import { describe, expect, it } from 'vitest'
import { noticeView, pickNotice, type LiveNotice } from './notices'
import { liveTabState, stateChip } from './state'

const ctx = { fileName: 'rough-cut', kept: 'who’s watching, the changes and your playhead' }
const ids = (n: LiveNotice): string[] => noticeView(n, ctx).actions.map((a) => a.id)
const labels = (n: LiveNotice): string[] => noticeView(n, ctx).actions.map((a) => a.label)

describe('outside saves', () => {
  it('taken in', () => {
    const v = noticeView({ kind: 'outside_save', save: { outcome: 'ingested', rev: 31 } }, ctx)
    expect(v).toMatchObject({ tone: 'info', role: 'status', title: 'Saved outside K2 · now in the live file' })
    expect(v.line).toBe('The changes show as an outside change.')
    expect(labels({ kind: 'outside_save', save: { outcome: 'ingested', rev: 31 } })).toEqual(['Dismiss'])
  })

  it('kept both: open the copy (by its doc when the host gives one, else its path), History, Dismiss', () => {
    const save = { outcome: 'kept_both' as const, rev: 32, kept: 30, copy: { name: 'rough-cut (conflict Atlas).otio', path: '/ws/edits/rough-cut (conflict Atlas).otio' } }
    const v = noticeView({ kind: 'outside_save', save }, ctx)
    expect(v).toMatchObject({ tone: 'warn', role: 'alert', title: 'Saved outside K2: that save is now live.', line: 'What was live before is kept as a copy.' })
    expect(v.actions.map((a) => [a.id, a.label, !!a.primary])).toEqual([
      ['open-copy', 'Open the copy', true],
      ['history', 'History', false],
      ['dismiss', 'Dismiss', false],
    ])
    expect(v.text).not.toContain('conflict')
    // No copy written: History holds it, and there is nothing to open.
    const noCopy = noticeView({ kind: 'outside_save', save: { ...save, copy: null } }, ctx)
    expect(noCopy.line).toBe('What was live before is kept in History.')
    expect(noCopy.actions.map((a) => a.id)).toEqual(['history', 'dismiss'])
  })

  it('declined: edits pause, and the saved versions are the next step', () => {
    const v = noticeView({ kind: 'outside_save', save: { outcome: 'declined', code: 'not_a_timeline', message: 'The save isn’t a timeline K2 can read.' } }, ctx)
    expect(v).toMatchObject({ tone: 'bad', role: 'alert', title: 'Saved outside K2, and the file can’t be live any more.' })
    expect(v.line).toBe('The save isn’t a timeline K2 can read. Edits are paused.')
    expect(ids({ kind: 'outside_save', save: { outcome: 'declined', code: 'x', message: '' } })).toEqual(['open-saved-file'])
    expect(noticeView({ kind: 'outside_save', save: { outcome: 'declined', code: 'x', message: '' } }, ctx).line).toBe('K2 can’t watch this save live. Edits are paused.')
  })

  it('missing: the live version is kept, save it again', () => {
    const v = noticeView({ kind: 'outside_save', save: { outcome: 'missing' } }, ctx)
    expect(v).toMatchObject({ tone: 'warn', role: 'alert', title: 'rough-cut was moved or deleted outside K2.', line: 'The live version is kept. Save it again to keep working on it.' })
    expect(labels({ kind: 'outside_save', save: { outcome: 'missing' } })).toEqual(['Save it again'])
  })
})

describe('connection', () => {
  it('reconnecting keeps what you see, with Retry now', () => {
    const v = noticeView({ kind: 'connection', state: 'reconnecting' }, ctx)
    expect(v).toMatchObject({ tone: 'info', role: 'status', title: 'Reconnecting…', busy: true })
    expect(v.line).toBe('What you see stays as it was: who’s watching, the changes and your playhead. Changes made meanwhile come in when you’re back.')
    expect(labels({ kind: 'connection', state: 'reconnecting' })).toEqual(['Retry now'])
  })

  it('trying: a disabled “Trying…”', () => {
    const v = noticeView({ kind: 'connection', state: 'trying' }, ctx)
    expect(v.line).toBe('Trying again now…')
    expect(v.actions).toEqual([{ id: 'retry', label: 'Trying…', disabled: true }])
  })

  it('can’t reconnect: Retry and the saved versions; says when a retry failed', () => {
    const v = noticeView({ kind: 'connection', state: 'cant' }, ctx)
    expect(v).toMatchObject({ tone: 'bad', role: 'alert', title: 'Can’t reach this file’s server.' })
    expect(v.line).toBe('K2 stopped trying. You see things as they were; anything new shows when you’re back.')
    expect(v.actions.map((a) => [a.label, !!a.primary])).toEqual([['Retry', true], ['Open the saved file', false]])
    expect(noticeView({ kind: 'connection', state: 'cant', retried: true }, ctx).line).toMatch(/^Tried again just now: still no answer\. /)
  })

  it('offline', () => {
    const v = noticeView({ kind: 'connection', state: 'offline' }, ctx)
    expect(v).toMatchObject({ tone: 'warn', title: 'You’re offline.', line: 'You see things as they were; anything new shows when you’re back online.' })
    expect(labels({ kind: 'connection', state: 'offline' })).toEqual(['Retry'])
  })

  it('back: says what came in, and whom you follow again', () => {
    expect(noticeView({ kind: 'connection', state: 'back', changes: 0 }, ctx).line).toBe('Nothing changed while you were away.')
    expect(noticeView({ kind: 'connection', state: 'back', changes: 1, following: 'Atlas' }, ctx).line).toBe('You’re following Atlas again. 1 change came in while you were away. It’s first in Changes.')
    expect(noticeView({ kind: 'connection', state: 'back', changes: 3 }, ctx).line).toBe('3 changes came in while you were away. They’re first in Changes.')
    expect(labels({ kind: 'connection', state: 'back', changes: 0 })).toEqual(['Dismiss'])
  })
})

describe('a refusal', () => {
  it('names who held it, and asks them', () => {
    const v = noticeView({ kind: 'refused', actor: 'Atlas', verb: 'trim', target: 'Interview B', holder: 'Brook' }, ctx)
    expect(v.title).toBe('Atlas couldn’t trim Interview B: Brook is holding it.')
    expect(v.actions.map((a) => a.label)).toEqual(['Ask Brook', 'Dismiss'])
  })
})

describe('every notice', () => {
  const all: LiveNotice[] = [
    { kind: 'outside_save', save: { outcome: 'ingested', rev: 1 } },
    { kind: 'outside_save', save: { outcome: 'kept_both', rev: 2, kept: 1, copy: { name: 'a', path: '/a' } } },
    { kind: 'outside_save', save: { outcome: 'declined', code: 'x', message: 'm' } },
    { kind: 'outside_save', save: { outcome: 'missing' } },
    { kind: 'refused', actor: 'Atlas', verb: 'move', target: 'Vase', holder: 'Brook' },
    ...(['reconnecting', 'trying', 'cant', 'offline', 'back'] as const).map((state) => ({ kind: 'connection' as const, state })),
  ]
  it('offers a next step and names no vendor app', () => {
    for (const n of all) {
      const v = noticeView(n, ctx)
      expect(v.actions.length, JSON.stringify(n)).toBeGreaterThan(0)
      expect(v.text).not.toMatch(/blender|otio|premiere|resolve|final cut/i)
    }
  })
})

describe('one notice at a time', () => {
  it('connection first, then the newest of the rest; dismissed ones step aside', () => {
    const save: LiveNotice = { kind: 'outside_save', save: { outcome: 'missing' } }
    const refusal: LiveNotice = { kind: 'refused', actor: 'Atlas', verb: 'trim', target: 'B', holder: 'Brook' }
    expect(pickNotice({ connection: { kind: 'connection', state: 'reconnecting' }, notices: [save, refusal] })).toEqual({ kind: 'connection', state: 'reconnecting' })
    expect(pickNotice({ connection: null, notices: [save, refusal] })).toBe(refusal)
    expect(pickNotice({ connection: null, notices: [save, refusal], dismissed: (n) => n === refusal })).toBe(save)
    expect(pickNotice({ connection: null, notices: [] })).toBeNull()
  })
})

describe('the header’s state word', () => {
  it('one dot and one word per state', () => {
    expect(stateChip('live')).toEqual({ word: 'Live', tone: 'ok' })
    expect(stateChip('live-behind')).toEqual({ word: 'Live · picture behind', tone: 'warn' })
    expect(stateChip('loading')).toEqual({ word: 'Loading', tone: 'muted' })
    expect(stateChip('reconnecting')).toEqual({ word: 'Reconnecting', tone: 'warn' })
    expect(stateChip('cant')).toEqual({ word: 'Can’t reconnect', tone: 'danger' })
    expect(stateChip('offline')).toEqual({ word: 'Offline', tone: 'muted' })
    expect(stateChip('paused')).toEqual({ word: 'Edits paused', tone: 'warn' })
    expect(stateChip('missing')).toEqual({ word: 'File missing', tone: 'warn' })
    expect(stateChip('read-only')).toEqual({ word: 'Read only (saved version)', tone: 'muted' })
  })

  it('comes from the tier, the connection and the last outside save', () => {
    expect(liveTabState({ tier: 'loading', connection: null, outsideSave: null })).toBe('loading')
    expect(liveTabState({ tier: 'live', connection: null, outsideSave: null })).toBe('live')
    expect(liveTabState({ tier: 'live', connection: null, outsideSave: null, behind: true })).toBe('live-behind')
    expect(liveTabState({ tier: 'live', connection: 'trying', outsideSave: null })).toBe('reconnecting')
    expect(liveTabState({ tier: 'live', connection: 'cant', outsideSave: null })).toBe('cant')
    expect(liveTabState({ tier: 'live', connection: 'back', outsideSave: null })).toBe('live')
    expect(liveTabState({ tier: 'live', connection: null, outsideSave: { outcome: 'declined', code: 'x', message: '' } })).toBe('paused')
    expect(liveTabState({ tier: 'live', connection: null, outsideSave: { outcome: 'missing' } })).toBe('missing')
    expect(liveTabState({ tier: 'live', connection: null, outsideSave: { outcome: 'ingested', rev: 2 } })).toBe('live')
    expect(liveTabState({ tier: 'saved-only', connection: null, outsideSave: null })).toBe('read-only')
  })
})
