// The save model's reducer: every transition, and the invariant that a failed
// write never clears the tab's dot.

import { describe, expect, it } from 'vitest'
import { canAutosave, INITIAL_SAVE, saveReducer, saveStatus, type SaveEvent, type SavePhase, type SaveState } from './reducer'

const run = (...events: SaveEvent[]): SaveState => events.reduce(saveReducer, INITIAL_SAVE)
const edit: SaveEvent = { type: 'edit', dirty: true }
const save: SaveEvent = { type: 'save' }
const fail: SaveEvent = { type: 'failed', reason: 'the drive is full' }

/** Every phase, reached the way the editor reaches it. */
const AT: Record<SavePhase, SaveEvent[]> = {
  clean: [],
  dirty: [edit],
  saving: [edit, save],
  saved: [edit, save, { type: 'saved', dirty: false }],
  failed: [edit, save, fail],
  'failed-twice': [edit, save, fail, save, fail],
  conflict: [edit, save, { type: 'conflict', code: 'file_missing' }],
}

describe('saveReducer: transitions', () => {
  it('starts clean, with no dot', () => {
    expect(INITIAL_SAVE).toMatchObject({ phase: 'clean', dirty: false, failures: 0 })
  })

  it('every phase is reachable as described', () => {
    for (const [phase, events] of Object.entries(AT)) expect(run(...events).phase).toBe(phase)
  })

  it('clean → dirty on an edit; typing back to the file is clean again', () => {
    expect(run(edit)).toMatchObject({ phase: 'dirty', dirty: true })
    expect(run(edit, { type: 'edit', dirty: false })).toMatchObject({ phase: 'clean', dirty: false })
  })

  it('dirty → saving → saved, and the dot goes', () => {
    expect(run(edit, save)).toMatchObject({ phase: 'saving', dirty: true })
    expect(run(...AT.saved)).toMatchObject({ phase: 'saved', dirty: false, failures: 0 })
  })

  it('an edit while saving: the save lands and the buffer is still dirty', () => {
    const s = run(edit, save, edit)
    expect(s).toMatchObject({ phase: 'saving', dirty: true, editedWhileSaving: true })
    expect(saveReducer(s, { type: 'saved', dirty: true })).toMatchObject({ phase: 'dirty', dirty: true })
  })

  it('saved → dirty on the next edit', () => {
    expect(run(...AT.saved, edit)).toMatchObject({ phase: 'dirty', dirty: true })
  })

  it('first failure → failed, second → failed-twice, a third stays failed-twice', () => {
    expect(run(...AT.failed)).toMatchObject({ phase: 'failed', failures: 1, reason: 'the drive is full' })
    expect(run(...AT['failed-twice'])).toMatchObject({ phase: 'failed-twice', failures: 2 })
    expect(run(...AT['failed-twice'], save, fail)).toMatchObject({ phase: 'failed-twice', failures: 3 })
  })

  it('Retry after a failure saves; success clears the failures', () => {
    expect(run(...AT.failed, save)).toMatchObject({ phase: 'saving', failures: 1 })
    expect(run(...AT['failed-twice'], save, { type: 'saved', dirty: false })).toMatchObject({ phase: 'saved', failures: 0, reason: null })
  })

  it('after a failure autosave waits for a new edit (no retry loop); Retry still saves', () => {
    expect(canAutosave(run(...AT.failed))).toBe(false)
    expect(canAutosave(run(...AT['failed-twice']))).toBe(false)
    expect(canAutosave(run(...AT.failed, edit))).toBe(true)
    expect(canAutosave(run(...AT.failed, edit, save, fail))).toBe(false)
    expect(run(...AT.failed, save).phase).toBe('saving')
  })

  it('an edit after a failure keeps the bar (the next save is the second try)', () => {
    expect(run(...AT.failed, edit)).toMatchObject({ phase: 'failed', dirty: true, failures: 1 })
    expect(run(...AT.failed, edit, save, fail).phase).toBe('failed-twice')
  })

  it('a conflict K2 can’t merge stops autosave until the person acts', () => {
    const s = run(...AT.conflict)
    expect(s).toMatchObject({ phase: 'conflict', conflict: 'file_missing', dirty: true })
    expect(canAutosave(s)).toBe(false)
    expect(canAutosave(saveReducer(s, edit))).toBe(false)
    // "Save it again" is a save.
    expect(saveReducer(s, save).phase).toBe('saving')
  })

  it('merged: the merged buffer is unsaved and autosaves', () => {
    const s = run(edit, save, { type: 'merged' })
    expect(s).toMatchObject({ phase: 'dirty', dirty: true })
    expect(canAutosave(s)).toBe(true)
  })

  it('reset (Undo a restore, Use their version, Reload): clean', () => {
    for (const events of Object.values(AT)) expect(run(...events, { type: 'reset' })).toMatchObject({ phase: 'clean', dirty: false, failures: 0, conflict: null })
  })

  it('restored: a draft came back, so it is dirty and autosaves', () => {
    const s = run({ type: 'restored' })
    expect(s).toMatchObject({ phase: 'dirty', dirty: true })
    expect(canAutosave(s)).toBe(true)
  })

  it('a save with nothing to save is ignored', () => {
    expect(run(save)).toBe(INITIAL_SAVE)
    expect(run(edit, save, save).phase).toBe('saving')
  })

  it('read-only and locked files never autosave', () => {
    expect(canAutosave(run(edit), { locked: true })).toBe(false)
    expect(canAutosave(run(edit))).toBe(true)
  })
})

describe('saveReducer: a failed write never clears the dot', () => {
  const events: SaveEvent[] = [
    edit,
    { type: 'edit', dirty: false },
    save,
    { type: 'saved', dirty: true },
    { type: 'saved', dirty: false },
    fail,
    { type: 'conflict', code: 'file_missing' },
    { type: 'conflict', code: 'file_changed' },
    { type: 'merged' },
    { type: 'restored' },
  ]
  for (const [phase, path] of Object.entries(AT)) {
    it(`from ${phase}: a failure or conflict keeps the dot`, () => {
      const before = run(...path)
      for (const bad of [fail, { type: 'conflict', code: 'file_changed' } as SaveEvent, { type: 'conflict', code: 'file_missing' } as SaveEvent]) {
        if (!before.dirty) continue // nothing unsaved: no write to fail
        expect(saveReducer(before, bad).dirty).toBe(true)
      }
    })
  }
  it('across every sequence of three events, only a landed save, a reset or typing back clears the dot', () => {
    for (const a of events)
      for (const b of events)
        for (const c of events) {
          let s = INITIAL_SAVE
          for (const e of [a, b, c]) {
            const next = saveReducer(s, e)
            if (s.dirty && !next.dirty) expect(['saved', 'reset', 'edit']).toContain(e.type)
            if (e.type === 'failed' || e.type === 'conflict') expect(next.dirty || !s.dirty).toBe(true)
            s = next
          }
        }
  })
})

describe('saveStatus: the status line agrees with the bar', () => {
  it('words for every phase', () => {
    expect(saveStatus(run())).toEqual({ word: 'Saved', tone: 'ok' })
    expect(saveStatus(run(...AT.dirty))).toEqual({ word: 'Saving in a moment', tone: 'pending' })
    expect(saveStatus(run(...AT.saving))).toEqual({ word: 'Saving', tone: 'pending' })
    expect(saveStatus(run(...AT.saved))).toEqual({ word: 'Saved', tone: 'ok' })
    expect(saveStatus(run(...AT.failed))).toEqual({ word: 'Not saved', tone: 'bad' })
    expect(saveStatus(run(...AT['failed-twice']))).toEqual({ word: 'Not saved', tone: 'bad' })
    expect(saveStatus(run(...AT.conflict))).toEqual({ word: 'Not saved: the file was deleted or moved', tone: 'bad' })
    expect(saveStatus(run(edit, save, { type: 'conflict', code: 'file_changed' }))?.word).toBe('Not saved: the file changed on disk')
  })
  it('never says saved during a conflict', () => {
    for (const code of ['file_missing', 'file_changed', 'file_exists'] as const) expect(saveStatus(run(edit, save, { type: 'conflict', code }))?.word).toMatch(/^Not saved/)
  })
  it('after a merge that saved: Merged · Saved', () => {
    expect(saveStatus(run(...AT.saved), { merged: true })).toEqual({ word: 'Merged · Saved', tone: 'ok' })
  })
  it('read only: no save word', () => {
    expect(saveStatus(run(), { locked: true })).toBeNull()
  })
})
