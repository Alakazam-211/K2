// "Say something to Atlas" on a live text (build-editors.md E4b;
// wire-shapes-k2-2026-11-15 "Say something"). LiveDocSource.say posts
// `docs/say {text, to, at}` and the socket's `say` frames become Messages.
// Pinned here:
//   - Send posts the words, whom they're to and what they point at; the sent
//     line joins Messages at once (by the route's id, so its own frame isn't
//     shown twice);
//   - a reply frame from the agent joins Messages, addressed to you;
//   - the route's refusals in words: rate limited (with the wait), not in the
//     room, an unknown person, no live room, and a dropped connection;
//   - the say row's failure line carries those words.

import { describe, expect, it, vi } from 'vitest'
import type { RosterEntry } from '@k2/mpc-client'
import type { LiveEditorEvents } from '@/components/LiveEditor'
import { sayReducer, sayView, SAY_IDLE } from '@/lib/live-view/shared/say'
import { LiveDocSource, type LiveDocEditor } from './live-doc-source'
import { sayFailure } from './say-words'
import type { TappedWelcome, TransportTap } from './transport-tap'

class Editor implements LiveDocEditor {
  text = '# Harbor\n\n## Setup\n\nClone it.\n'
  state: LiveDocEditor['state'] = 'live'
  rights = 'edit'
  roster: RosterEntry[] = [
    { handle: 'me', name: 'You', colour: '#2563eb', kind: 'person', role: 'editor', away: false, self: true },
    { handle: 'atlas', name: 'Atlas', colour: '#7c3aed', kind: 'agent', role: 'editor', away: false, cursor: { root: 'source', anchor: 0, head: 0, head_lc: '5:1' } },
  ]
  pending = 0
  undo = vi.fn()
  redo = vi.fn()
  reconnectNow = vi.fn()
  private cbs = new Map<string, Set<(v: unknown) => void>>()
  on<K extends keyof LiveEditorEvents>(event: K, cb: (v: LiveEditorEvents[K]) => void): () => void {
    const set = this.cbs.get(event) ?? new Set()
    set.add(cb as (v: unknown) => void)
    this.cbs.set(event, set)
    return () => void set.delete(cb as (v: unknown) => void)
  }
  emit<K extends keyof LiveEditorEvents>(event: K, v: LiveEditorEvents[K]): void {
    for (const cb of this.cbs.get(event) ?? []) cb(v)
  }
}

class Tap implements TransportTap {
  transport = {} as TransportTap['transport']
  private w = new Set<(w: TappedWelcome) => void>()
  private f = new Set<(f: Record<string, unknown>) => void>()
  onWelcome(cb: (w: TappedWelcome) => void): () => void {
    this.w.add(cb)
    return () => void this.w.delete(cb)
  }
  onFrame(cb: (f: Record<string, unknown>) => void): () => void {
    this.f.add(cb)
    return () => void this.f.delete(cb)
  }
  welcome(): void {
    for (const cb of this.w) cb({ root: 'source', version: 1, client: 7, self: 'me', rights: 'edit', features: ['view_address'] } as TappedWelcome)
  }
  frame(f: Record<string, unknown>): void {
    for (const cb of this.f) cb(f)
  }
}

const httpError = (status: number, code: string | null, body: Record<string, unknown> = {}) => Object.assign(new Error(String(body.error ?? code ?? status)), { status, code, body })

function setup(post: (b: Record<string, unknown>) => Promise<unknown>) {
  const src = new LiveDocSource({ markdown: true, online: true, post })
  const tap = new Tap()
  const ed = new Editor()
  src.attachTap(tap)
  src.attach(ed)
  tap.welcome()
  ed.emit('state', 'live')
  return { src, tap }
}

describe('say', () => {
  it('posts the words, whom to and what they point at, and shows the sent line once', async () => {
    const post = vi.fn(async () => ({ ok: true, say: { id: 'say-1', at_ms: 5000, to: 'atlas' }, sockets: 1, inbox: true }))
    const { src, tap } = setup(post)
    expect(src.snapshot().features.say).toBe(true)
    const r = await src.say({ to: 'atlas', text: 'Keep step 3 short, please', address: '@h:setup' })
    expect(r).toEqual({ ok: true })
    expect(post).toHaveBeenCalledWith({ text: 'Keep step 3 short, please', to: 'atlas', at: '@h:setup' })
    // The chip keeps the heading's own words; only the wire carries the slug.
    expect(src.snapshot().messages).toEqual([{ id: 'say-1', from: 'me', to: 'atlas', text: 'Keep step 3 short, please', at_ms: 5000, address: { label: 'Setup', address: '@h:setup' } }])
    // Its own frame comes back on the socket: not shown twice.
    tap.frame({ type: 'say', id: 'say-1', from: { id: 'me', name: 'You', kind: 'person', colour: '#2563eb' }, to: { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }, text: 'Keep step 3 short, please', at: '@h:setup', time: null, reply_to: null, at_ms: 5000 })
    expect(src.snapshot().messages).toHaveLength(1)
  })

  it('reads a frame’s address in either form (and the old one, for one release)', () => {
    const { src, tap } = setup(async () => ({}))
    const from = { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }
    tap.frame({ type: 'say', id: 'a', from, to: null, text: 'x', at: 'L5', time: null, reply_to: null, at_ms: 1 })
    tap.frame({ type: 'say', id: 'b', from, to: null, text: 'y', at: 'heading:Setup', time: null, reply_to: null, at_ms: 2 })
    expect(src.snapshot().messages.map((m) => m.address)).toEqual([
      { label: 'line 5', address: 'L5' },
      { label: 'Setup', address: 'heading:Setup' },
    ])
  })

  it('takes a reply frame into Messages, addressed to you', () => {
    const { src, tap } = setup(async () => ({}))
    tap.frame({ type: 'say', id: 'say-2', from: { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }, to: { id: 'me', name: 'You', kind: 'person', colour: '#2563eb' }, text: 'Will do. One line, with the command.', at: null, time: null, reply_to: 'say-1', at_ms: 6000 })
    expect(src.snapshot().messages).toEqual([{ id: 'say-2', from: 'atlas', to: 'me', text: 'Will do. One line, with the command.', at_ms: 6000, address: null }])
  })

  it('reads a frame’s time as the playhead’s time position, and (this release) the old bare ticks', () => {
    const { src, tap } = setup(async () => ({}))
    const from = { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }
    const to = { id: 'me', name: 'You', kind: 'person', colour: '#2563eb' }
    tap.frame({ type: 'say', id: 'say-3', from, to, text: 'Here', at: '@seq:S2', time: { at: '1200', in: '@seq:S2' }, reply_to: null, at_ms: 7000 })
    tap.frame({ type: 'say', id: 'say-4', from, to, text: 'And here', at: '@item:5', time: '2400', reply_to: null, at_ms: 7100 })
    expect(src.snapshot().messages.map((m) => m.time)).toEqual([{ at: '1200', in: '@seq:S2' }, { at: '2400' }])
  })

  it('says why it could not send: rate limited, with the wait', async () => {
    const { src } = setup(async () => {
      throw httpError(429, 'rate_limited', { error: 'rate limited', retryAfterMs: 2400 })
    })
    const r = await src.say({ to: 'atlas', text: 'hi', address: null })
    expect(r).toEqual({ ok: false, reason: 'you sent several at once; you can send again in 3 s', retryAfterMs: 2400 })
    expect(src.snapshot().messages).toEqual([])
  })

  it('names the other refusals plainly', () => {
    const names = (id: string) => (id === 'atlas' ? 'Atlas' : null)
    expect(sayFailure(httpError(409, 'not_in_room'), 'atlas', names).reason).toBe('you’re not in this file’s live session now; reopen the file')
    expect(sayFailure(httpError(409, 'no_live_room'), 'atlas', names).reason).toBe('this file has no live session to send in')
    expect(sayFailure(httpError(404, null), 'atlas', names).reason).toBe('Atlas isn’t in this file now')
    expect(sayFailure(new Error('Failed to fetch'), 'atlas', names).reason).toBe('the server can’t be reached')
    expect(sayFailure(httpError(429, 'rate_limited', {}), 'atlas', names).reason).toBe('you sent several at once; you can send again in a few seconds')
  })

  it('puts the reason in the say row, keeping the words', () => {
    let s = sayReducer({ ...SAY_IDLE, draft: 'hi' }, { type: 'send' })
    s = sayReducer(s, { type: 'failed', reason: 'you sent several at once; you can send again in 3 s' })
    const v = sayView(s, { toName: 'Atlas', offline: false, paused: false })
    expect(v.line).toEqual({ tone: 'bad', text: 'Couldn’t send: you sent several at once; you can send again in 3 s. Your message is kept here.', action: 'Send again' })
    expect(s.draft).toBe('hi')
  })
})
