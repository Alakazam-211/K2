// The monitor and transport, pure (T4): every state of the video player
// design section reached on the sample, with its words and actions; your
// own playback (the watching keys, the end, loops); the scrub bar's marks
// and its frames; and whose playhead the transport names.

import { describe, expect, it } from 'vitest'
import { advance, KEY_MAP, keysHint, playbackKey, type KeyContext } from './keys'
import { detailRows, NOT_ON_THIS_HOST } from './monitor-details'
import { CUTS_TITLE, monitorView, PREP_PLAY_TIP, STILLS_DECODE, type LocalPlay, type MonitorView } from './monitor-view'
import { onYourPlayhead } from './sample-source'
import { PREP, STATES, WATCH } from './scenarios'
import { simulate } from './sim'
import { modelOf } from './fold'
import { nowOf } from './snapshot'
import { editStarts, scrubAt, scrubFrame, scrubMarks, whoLine } from './transport'
import { CTX, FACTS } from './world'

const MS = onYourPlayhead(STATES, 'states')
const at = (label: string): number => MS.steps.findIndex((s) => s.label === label)

function viewOf(sc = MS, i: number, local: LocalPlay | null = null, whose: string | null = null): MonitorView {
  const S = simulate(sc, i, CTX)
  return monitorView({ S, M: modelOf(S.B), facts: FACTS, now: nowOf(S), whose, local })
}
const step = (label: string, local: LocalPlay | null = null): MonitorView => viewOf(MS, at(label), local)

describe('every monitor state in the design section, with its words', () => {
  it('paused without the exact frame: the nearest picture, never a later one, and how far behind', () => {
    const v = step('still')
    expect(v.state).toBe('paused')
    expect(v.exact).toBe(false)
    expect(v.paint.how).toBe('tile')
    expect(v.note?.text).toMatch(/^Nearest picture: \d\.\d s before$/)
    expect(v.label).toEqual({ lane: 'V1', name: 'Interview C' })
  })

  it('before the first grid tile: the head; the poster only until the head exists', () => {
    expect(step('still · head').paint.how).toBe('tile')
    const p = step('still · poster')
    expect(p.paint.how).toBe('poster')
    expect(p.note?.text).toBe('Clip’s poster')
  })

  it('loading the first frame: the poster dimmed, a spinner, “Loading…” after 300 ms', () => {
    const v = step('loading')
    expect(v.state).toBe('loading')
    expect(v.paint.how).toBe('dim')
    expect(v.spinner).toBe(true)
    expect(v.note).toMatchObject({ text: 'Loading…', late: true })
  })

  it('playing: frames, no note (the mode word says cuts only), Pause on the button', () => {
    const v = step('playing')
    expect(v.state).toBe('playing')
    expect(v.paint.how).toBe('frame')
    expect(v.note).toBeNull()
    expect(v.mode).toEqual({ word: 'Plays cuts only', title: CUTS_TITLE })
    expect(v.playTip).toBe('Pause (Space or K)')
  })

  it('seeking: a quick seek shows nothing new; a longer one keeps a soft picture and says “Seeking…” late, no rung', () => {
    const q = step('quick seek')
    expect(q.state).toBe('seeking')
    expect(q.note).toBeNull()
    const v = step('seeking')
    expect(v.state).toBe('seeking')
    expect(v.paint.how).toBe('soft')
    expect(v.note).toMatchObject({ text: 'Seeking…', late: true, spin: true })
    expect(v.note?.text).not.toMatch(/px|640/)
  })

  it('ready: paused on the exact frame, nothing on the picture but its label', () => {
    const v = step('sharp')
    expect(v.state).toBe('paused')
    expect(v.exact).toBe(true)
    expect(v.paint.how).toBe('frame')
    expect(v.note).toBeNull()
    expect(v.playTip).toBe('Play (Space or L)')
  })

  it('waiting mid-play: the last frame holds, play pauses itself, “Waiting for video…”', () => {
    const v = step('waiting')
    expect(v.state).toBe('waiting')
    expect(v.head.playing).toBe(false)
    expect(v.note).toMatchObject({ text: 'Waiting for video…', late: true, spin: true })
  })

  it('stills only for you: the mode word is Stills and says why; Play is off', () => {
    const v = step('stills only')
    expect(v.state).toBe('stills')
    expect(v.mode).toEqual({ word: 'Stills', title: STILLS_DECODE })
    expect(v.canPlay).toBe(false)
    expect(v.playTip).toBe(STILLS_DECODE)
    expect(v.card).toBeNull()
  })

  it('not a video: a card with the name and its kind, and Skip to the next clip', () => {
    const v = step('not a video')
    expect(v.state).toBe('nomedia')
    expect(v.card).toMatchObject({ title: 'Map still', kindWord: 'Image', line: '' })
    expect(v.card?.next).not.toBeNull()
    expect(v.paint.how).toBe('none')
  })

  it('no preview (not_on_this_host): the card says so; its code is only in details', () => {
    const v = step('no preview')
    expect(v.state).toBe('no_preview')
    expect(v.card).toMatchObject({ title: 'Interview C', line: 'This clip can’t be previewed.' })
    expect(JSON.stringify(v.card)).not.toContain(NOT_ON_THIS_HOST)
    const S = simulate(MS, at('no preview'), CTX)
    const rows = detailRows(S, modelOf(S.B), v, 'rough-cut')
    expect(rows).toContainEqual(['Code', NOT_ON_THIS_HOST])
    expect(rows).toContainEqual(['Why', 'the computer that holds the file can’t read it'])
  })

  it('offline, nest and the gap each say what they are, with the next step', () => {
    expect(step('offline').card).toMatchObject({ kind: 'offline', line: 'This clip’s media isn’t available right now.' })
    expect(step('nest').card).toMatchObject({ kind: 'nest', line: 'A nested timeline: the preview shows cuts at the top level only.' })
    const g = step('gap')
    expect(g.state).toBe('gap')
    expect(g.gap?.line).toBe('Gap: nothing on the video tracks here')
    expect(g.gap?.next).not.toBeNull()
  })

  it('other items are passed over: the clip under them shows', () => {
    const v = step('other')
    expect(v.picture?.skipped.length).toBeGreaterThan(0)
    expect(v.card).toBeNull()
  })

  it('the end: the last frame holds, “End of the edit” with Play from the start; Space plays from the start', () => {
    const v = step('end')
    expect(v.state).toBe('ended')
    expect(v.note).toMatchObject({ text: 'End of the edit', late: false, restart: true })
    expect(v.playTip).toBe('Play from the start (Space)')
    expect(playbackKey('play', null, ctx(v))).toEqual({ f: 0, rate: 1 })
  })

  it('preparing video: the count of previewable clips, the ones that can’t be previewed, play waits for the next 30 s', () => {
    const v = viewOf(PREP, 0)
    expect(v.state).toBe('preparing')
    expect(v.prep?.text).toMatch(/^Preparing video · 3 of \d+ previewable clips ready$/)
    expect(v.prep?.cant.map((c) => c.name)).toEqual(['B-roll 05', 'B-roll 08'])
    expect(v.canPlay).toBe(false)
    expect(v.playTip).toBe(PREP_PLAY_TIP)
    expect(playbackKey('play', null, ctx(v))).toBeNull()
    expect(viewOf(PREP, 2).canPlay).toBe(true)
    expect(viewOf(PREP, 4).prep?.hold).toBe('Preparing this clip…')
    expect(viewOf(PREP, 4).state).toBe('waiting')
    const done = viewOf(PREP, 5)
    expect(done.prep).toMatchObject({ text: 'Video ready', done: true })
  })

  it('every state in the design section is reached', () => {
    const seen = new Set<string>()
    MS.steps.forEach((_, i) => seen.add(viewOf(MS, i).state))
    PREP.steps.forEach((_, i) => seen.add(viewOf(PREP, i).state))
    for (const s of ['loading', 'playing', 'paused', 'seeking', 'waiting', 'ended', 'preparing', 'stills', 'no_preview', 'offline', 'nest', 'nomedia', 'gap']) expect(seen, s).toContain(s)
  })
})

function ctx(v: MonitorView, edits: number[] = []): KeyContext {
  return { f: v.head.f, playing: v.head.playing, canPlay: v.canPlay, ended: v.state === 'ended', endF: 2880, edits }
}

describe('your own playback', () => {
  const base: KeyContext = { f: 100, playing: false, canPlay: true, ended: false, endF: 500, edits: [40, 200, 300] }
  it('the mock’s key map: Space, J K L, arrows, Home End, = - \\, Esc', () => {
    expect(Object.keys(KEY_MAP).sort()).toEqual([' ', '+', '-', '=', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'End', 'Escape', 'Home', 'J', 'K', 'L', '\\', 'j', 'k', 'l'].sort())
    expect(keysHint('Atlas')).toContain('↑ ↓ Atlas’s edits')
  })
  it('Space plays and pauses; L goes forward and again for 2×, 4×, 8×; J back; K stops', () => {
    const p = playbackKey('play', null, base)!
    expect(p).toEqual({ f: 100, rate: 1 })
    expect(playbackKey('play', p, base)).toEqual({ f: 100, rate: 0 })
    let l = playbackKey('forward', null, base)!
    for (const r of [2, 4, 8, 8]) expect((l = playbackKey('forward', l, base)!).rate).toBe(r)
    expect(playbackKey('back', null, base)!.rate).toBe(-1)
    expect(playbackKey('back', { f: 9, rate: -1 }, base)!.rate).toBe(-2)
    expect(playbackKey('stop', l, base)!.rate).toBe(0)
  })
  it('← → a frame, ↑ ↓ the agent’s edits, Home and End', () => {
    expect(playbackKey('frame-', null, base)!.f).toBe(99)
    expect(playbackKey('frame+', null, base)!.f).toBe(101)
    expect(playbackKey('edit-', null, base)!.f).toBe(40)
    expect(playbackKey('edit+', null, base)!.f).toBe(200)
    expect(playbackKey('edit-', null, { ...base, f: 10 })).toBeNull()
    expect(playbackKey('start', null, base)!.f).toBe(0)
    expect(playbackKey('end', null, base)!.f).toBe(499)
  })
  it('playback stops at the end and says so; a loop goes round; a key leaves the loop', () => {
    expect(advance({ f: 490, rate: 1 }, 1, 24, 500)).toMatchObject({ f: 499, rate: 0, ended: true })
    expect(advance({ f: 10, rate: -2 }, 1, 24, 500)).toMatchObject({ f: 0, rate: 0 })
    const loop = { f0: 100, f1: 140, who: 'Atlas', verb: 'clip trim' }
    expect(advance({ f: 139, rate: 1, loop }, 0.1, 24, 500)).toMatchObject({ f: 100, loop })
    expect(playbackKey('frame+', { f: 120, rate: 1, loop }, base)!.loop).toBeUndefined()
  })
  it('your playback replaces the scenario’s: past the end it is the end state', () => {
    const v = step('sharp', { f: 2000, rate: 0, ended: true })
    expect(v.state).toBe('ended')
    expect(v.head.self).toBe(true)
    // the end wins over what its last frame shows: a gap there still says End of the edit
    const S = simulate(MS, at('gap'), CTX)
    const M = modelOf(S.B)
    const g = monitorView({ S, M, facts: FACTS, now: nowOf(S), local: { f: M.endF - 1, rate: 0, ended: true } })
    expect(g.state).toBe('ended')
    expect(g.note?.text).toBe('End of the edit')
  })
})

describe('the transport', () => {
  it('names whose playhead: yours by default; whoever `whose` names (T5’s hook); the loop and a skip', () => {
    const S = simulate(MS, at('sharp'), CTX)
    const M = modelOf(S.B)
    const mine = monitorView({ S, M, facts: FACTS, now: nowOf(S), local: null })
    expect(whoLine(S, mine, null)).toBe('Your playhead')
    const atlas = monitorView({ S, M, facts: FACTS, now: nowOf(S), local: null, whose: 'agent:atlas' })
    expect(atlas.head.self).toBe(false)
    expect(whoLine(S, atlas, null)).toBe('Atlas’s playhead')
    expect(whoLine(S, mine, { f: 3, rate: 4 })).toBe('Your playhead · 4×')
    expect(whoLine(S, mine, { f: 3, rate: 1, loop: { f0: 0, f1: 9, who: 'Atlas', verb: 'clip trim' } })).toBe('Looping Atlas’s trim · Esc to stop')
    expect(whoLine(S, mine, { f: 3, rate: 0, skipped: 'Interview B' })).toBe('Skipped to Interview B')
  })

  it('the scrub bar spans the whole edit: changes, markers, other playheads, the monitor’s head; a click lands on a frame', () => {
    const S = simulate(WATCH, 8, CTX)
    const M = modelOf(S.B)
    const v = monitorView({ S, M, facts: FACTS, now: nowOf(S), local: null })
    const m = scrubMarks(S, M, v, nowOf(S))
    expect(m.changes.length).toBeGreaterThan(0)
    expect(m.markers.length).toBeGreaterThan(0)
    expect(m.heads.map((h) => h.id)).not.toContain(v.head.id)
    expect(m.head).toBe(v.head.f)
    expect(scrubAt(M, 0)).toBe(0)
    expect(scrubAt(M, M.endF * 2)).toBe(1)
    expect(scrubFrame(M, 500, 1000)).toBe(Math.round(M.endF / 2))
    expect(scrubFrame(M, 2000, 1000)).toBe(Math.round(M.endF) - 1)
    expect(editStarts(S, 'agent:atlas').length).toBeGreaterThan(0)
  })
})
