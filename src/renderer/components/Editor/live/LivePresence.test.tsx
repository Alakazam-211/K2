// @vitest-environment jsdom

// Presence on a live text in the new editors (build-editors.md E4b), over a
// scripted `WS /cli/docs/live` (the kit's frames and K2's). Pinned here:
//   - holds from claim events: the label above the held lines ("Atlas is
//     rewriting Setup") with Message Atlas; the line under your caret inside
//     the hold; claim_ended takes it away; Message opens the say row to Atlas;
//   - Follow and Stop: "Follow Atlas" → "Following Atlas · Stop", the frame
//     in Atlas's colour, the say row; Stop, or your own typing, stops it;
//     clicking a Here row follows too;
//   - finished: Atlas's hold ends while you follow → "Atlas finished. Stopped
//     following." with Back to my place and Follow again;
//   - Say: Send posts docs/say with whom and where; rate limited says so and
//     keeps the words; a reply shows in the row and in the panel's Messages.
// The flags' placement and the band's sorting are pure (flags.test.ts,
// offscreen.test.ts): jsdom has no layout to measure.

import '../../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const post = vi.hoisted(() => vi.fn(async (_s: unknown, route: string, _b?: unknown): Promise<unknown> => ({ ok: true, route })))
vi.mock('@/lib/daemon-cli', async (orig) => ({ ...(await orig<typeof import('@/lib/daemon-cli')>()), daemonCliPost: post }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import type { LiveClientFrame, LiveClose, LiveServerFrame, LiveTransport, LiveWelcome } from '@/lib/docs-live-transport'
import type { ServerScope } from '@/kessel/server-scope'
import type { TappedWelcome } from '@/lib/editor/live/transport-tap'
import { LiveEditorFile } from './LiveEditorFile'

const SELF = 'person:owner'
const DOC = '# Harbor\n\nIntro.\n\n## Setup\n\nClone it.\n\n## Configuration\n\nA table.\n'

class ScriptedTransport implements LiveTransport {
  connects = 0
  sent: LiveClientFrame[] = []
  down = false
  private frameCbs = new Set<(f: LiveServerFrame) => void>()
  private closeCbs = new Set<(e: LiveClose) => void>()
  private isOpen = false
  constructor(private welcome: Partial<TappedWelcome> = {}) {}
  get open(): boolean {
    return this.isOpen
  }
  connect(): Promise<LiveWelcome> {
    this.connects += 1
    // While down, a reconnect never answers.
    if (this.down) return new Promise(() => undefined)
    this.isOpen = true
    return Promise.resolve({ root: 'source', version: 1, text: DOC, client: 100 + this.connects, self: SELF, rights: 'edit', features: ['view_address'], ...this.welcome } as LiveWelcome)
  }
  send(f: LiveClientFrame): void {
    if (this.isOpen) this.sent.push(f)
  }
  onFrame(cb: (f: LiveServerFrame) => void): () => void {
    this.frameCbs.add(cb)
    return () => void this.frameCbs.delete(cb)
  }
  onClose(cb: (e: LiveClose) => void): () => void {
    this.closeCbs.add(cb)
    return () => void this.closeCbs.delete(cb)
  }
  close(): void {
    this.isOpen = false
  }
  push(f: Record<string, unknown>): void {
    for (const cb of this.frameCbs) cb(f as LiveServerFrame)
  }
  end(e: LiveClose): void {
    this.isOpen = false
    for (const cb of this.closeCbs) cb(e)
  }
}

const scope = { connectionKey: 'test', hostKey: 'h', id: 's' } as unknown as ServerScope

function member(handle: string, name: string, o: Record<string, unknown> = {}): Record<string, unknown> {
  return { handle, name, colour: '#7c3aed', kind: 'person', role: 'editor', away: false, cursor: null, rights: 'edit', ...o }
}
/** A caret at the start of `text` in DOC (ASCII: bytes are characters). */
const caret = (text: string) => ({ root: 'source', anchor: DOC.indexOf(text), head: DOC.indexOf(text) })
const roster = (t: ScriptedTransport, members: Record<string, unknown>[]): void =>
  act(() => t.push({ type: 'roster_event', event: { kind: 'snapshot', version: 1, members, watching: 0 } }))
const everyone = [member(SELF, 'Rosson', { self: true }), member('agent:atlas', 'Atlas', { kind: 'agent', cursor: caret('Clone it.') }), member('person:brook', 'Brook', { away: true })]

async function open(t: ScriptedTransport, o: { onCoreOff?: () => void; width?: number } = {}) {
  const r = render(<LiveEditorFile scope={scope} filePath="/w/README.md" kind="markdown" roomReadOnly={null} onCoreOff={o.onCoreOff ?? (() => undefined)} width={o.width ?? 1440} createTransport={() => t} />)
  await waitFor(() => expect(screen.getByTestId('ed-live-file').dataset.liveState).toBe('live'))
  return r
}
import { EditorView } from "@codemirror/view"
const claim = (t: ScriptedTransport, e: Record<string, unknown>): void => act(() => t.push({ type: 'roster_event', event: e }))
const HOLD = { kind: 'claim_started', who: 'agent:atlas', claim: 'claim-1', lines: { root: 'source', first: 5, last: 7 }, note: 'rewriting Setup', hold: true, for_edit: true, expires_ms: 1, version: 1, actor: { id: 'agent:atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }, started_ms: 1 }
const ENDED = { kind: 'claim_ended', who: 'agent:atlas', claim: 'claim-1', how: 'released', version: 2, actor: { id: 'agent:atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }, at_ms: 2, at_rev: null }
const placeCaret = (text: string): void => {
  const content = document.querySelector('.cm-content') as HTMLElement
  const view = EditorView.findFromDOM(content)
  if (!view) throw new Error('no editor view')
  act(() => view.dispatch({ selection: { anchor: DOC.indexOf(text) } }))
}

beforeEach(() => {
  post.mockClear()
  window.localStorage.clear()
})
afterEach(() => cleanup())

describe('holds', () => {
  it('draws the label above the held lines, the line under your caret inside, and goes when the claim ends', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    claim(t, HOLD)
    const label = await screen.findByTestId('ed-hold-label')
    expect(label.textContent).toContain('Atlas is rewriting Setup')
    expect(within(label).getByRole('button', { name: 'Message Atlas' })).toBeTruthy()
    expect(screen.getByTestId('lv-sentence').textContent).toBe('Atlas is rewriting Setup')
    expect(screen.getByTestId('lv-person-agent:atlas').textContent).toContain('rewriting Setup')
    placeCaret('Clone it.')
    expect((await screen.findByTestId('ed-hold-hint')).textContent).toBe('Atlas is rewriting Setup. You can still type here; your words go in now, and Atlas sees them and keeps them.')
    placeCaret('A table.')
    await waitFor(() => expect(screen.queryByTestId('ed-hold-hint')).toBeNull())
    claim(t, ENDED)
    await waitFor(() => expect(screen.queryByTestId('ed-hold-label')).toBeNull())
  })

  it('Message Atlas opens the say row to Atlas', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    claim(t, HOLD)
    fireEvent.click(within(await screen.findByTestId('ed-hold-label')).getByRole('button', { name: 'Message Atlas' }))
    expect((await screen.findByTestId('lv-say-field')).getAttribute('placeholder')).toBe('Say something to Atlas…')
  })
})

describe('Follow', () => {
  it('Follow Atlas → Following Atlas · Stop, the frame and the say row; Stop ends it', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    fireEvent.click(await screen.findByTestId('lv-follow'))
    await waitFor(() => expect(screen.getByTestId('lv-follow').getAttribute('aria-label')).toBe('Following Atlas · Stop'))
    expect(screen.getByTestId('lv-follow').getAttribute('aria-pressed')).toBe('true')
    expect(document.querySelector('.ed-live-slot')?.hasAttribute('data-following')).toBe(true)
    expect(screen.getByTestId(`lv-person-${SELF}`).textContent).toContain('following Atlas')
    expect(screen.getByTestId('lv-say-field').getAttribute('placeholder')).toBe('Say something to Atlas…')
    fireEvent.click(screen.getByTestId('lv-follow'))
    await waitFor(() => expect(screen.getByTestId('lv-follow').textContent).toContain('Follow Atlas'))
    expect(screen.getByTestId('lv-follow').getAttribute('aria-pressed')).toBe('false')
    expect(document.querySelector('.ed-live-slot')?.hasAttribute('data-following')).toBe(false)
    expect(screen.queryByTestId('lv-say')).toBeNull()
  })

  it('your own typing stops it, and a Here row follows its person', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, [...everyone, member('person:sam', 'Sam', { cursor: caret('A table.') })])
    const row = within(await screen.findByTestId('ed-live-panel')).getByTestId('lv-person-person:sam')
    fireEvent.click(row.closest('button') ?? row.querySelector('button') ?? row)
    await waitFor(() => expect(screen.getByTestId('lv-follow').getAttribute('aria-label')).toBe('Following Sam · Stop'))
    fireEvent.keyDown(document.querySelector('.cm-content') as HTMLElement, { key: 'a' })
    await waitFor(() => expect(screen.getByTestId('lv-follow').getAttribute('aria-pressed')).toBe('false'))
  })

  it('when Atlas finishes: "Atlas finished. Stopped following.", Back to my place and Follow again', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    claim(t, HOLD)
    fireEvent.click(await screen.findByTestId('lv-follow'))
    await waitFor(() => expect(screen.getByTestId('lv-follow').getAttribute('aria-label')).toBe('Following Atlas · Stop'))
    claim(t, ENDED)
    await waitFor(() => expect(screen.getByTestId('lv-sentence').textContent).toBe('Atlas finished. Stopped following.'))
    const fin = screen.getByTestId('ed-follow-finished')
    expect(within(fin).getByText('Back to my place')).toBeTruthy()
    expect(screen.queryByTestId('lv-follow')).toBeNull()
    fireEvent.click(within(fin).getByText('Follow again'))
    await waitFor(() => expect(screen.getByTestId('lv-follow').getAttribute('aria-label')).toBe('Following Atlas · Stop'))
    expect(screen.queryByTestId('ed-follow-finished')).toBeNull()
  })
})

describe('Say something', () => {
  it('posts to docs/say with whom and where, and shows the reply in the row and in Messages', async () => {
    post.mockImplementation(async (_s: unknown, route: string) => (route === 'docs/say' ? { ok: true, say: { id: 'say-1', at_ms: 10, to: 'agent:atlas' }, sockets: 1, inbox: true } : { ok: true, route }))
    try {
      const t = new ScriptedTransport()
      await open(t)
      roster(t, everyone)
      fireEvent.click(await screen.findByTestId('lv-follow'))
      const field = await screen.findByTestId('lv-say-field')
      expect(screen.getByTestId('lv-say-chip').textContent).toBe('Setup')
      fireEvent.change(field, { target: { value: 'Keep step 3 short, please' } })
      fireEvent.keyDown(field, { key: 'Enter' })
      await waitFor(() => expect(post).toHaveBeenCalledWith(scope, 'docs/say', { path: '/w/README.md', text: 'Keep step 3 short, please', to: 'agent:atlas', at: '@h:setup' }))
      await waitFor(() => expect(screen.getByTestId('lv-say-last').textContent).toBe('You → Atlas: Keep step 3 short, please'))
      act(() => t.push({ type: 'say', id: 'say-2', from: { id: 'agent:atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed' }, to: { id: SELF, name: 'Rosson', kind: 'person', colour: null }, text: 'Will do. One line, with the command.', at: null, time: null, reply_to: 'say-1', at_ms: 20 }))
      await waitFor(() => expect(screen.getByTestId('lv-say-last').textContent).toBe('Atlas → You: Will do. One line, with the command.'))
      expect(screen.getAllByTestId('lv-message').map((m) => m.textContent?.split(':')[0])).toEqual(['Atlas → You', 'You → Atlas'])
    } finally {
      post.mockReset()
      post.mockImplementation(async (_s: unknown, route: string) => ({ ok: true, route }))
    }
  })

  it('rate limited: says so, keeps the words and offers Send again', async () => {
    post.mockImplementationOnce(async () => {
      throw Object.assign(new Error('rate limited'), { status: 429, code: 'rate_limited', body: { error: 'rate limited', retryAfterMs: 3000 } })
    })
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    fireEvent.click(await screen.findByTestId('lv-follow'))
    const field = await screen.findByTestId('lv-say-field')
    fireEvent.change(field, { target: { value: 'hello' } })
    fireEvent.keyDown(field, { key: 'Enter' })
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t send: you sent several at once; you can send again in 3 s. Your message is kept here.')
    expect(within(alert).getByText('Send again')).toBeTruthy()
    expect((screen.getByTestId('lv-say-field') as HTMLInputElement).value).toBe('hello')
  })
})
