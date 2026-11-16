// @vitest-environment jsdom

// A live file in the new editors (build-editors.md E4a): the kit's Live
// editor inside the markdown editor, over a scripted `WS /cli/docs/live`.
// Pinned here:
//   - alone: no Live chip, no avatar stack; the status line says "All changes saved";
//   - someone else here (an agent counts): the chip, the stack (you first, no
//     away people), the panel opens with "Here 2" and "Away 1";
//   - disconnected: the bar with the offline line, the Live tab's count struck,
//     the people "last seen…", Live edits off with its reason, the status line
//     "Saved on this computer";
//   - an outside save merged in: the bar; Use their version calls the route
//     with the save's rev, then offers Undo;
//   - a refusal: a Changes row with Ask <holder>;
//   - read only: opens in Read, Write off with "Ask <owner> for edit access";
//   - live off (the core turned off mid-session): onCoreOff, the editor stays.

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
const status = (): string => document.querySelector('.ed-status')?.textContent ?? ''

beforeEach(() => {
  post.mockClear()
  window.localStorage.clear()
})
afterEach(() => cleanup())

describe('alone and with others', () => {
  it('alone: no chip and no stack, the text is the live text, all changes saved', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, [member(SELF, 'Rosson', { self: true })])
    expect(screen.queryByTestId('lv-state')).toBeNull()
    expect(screen.queryByTestId('lv-stack')).toBeNull()
    expect(screen.getByTestId('editor-tab').getAttribute('data-mode')).toBe('write')
    await waitFor(() => expect(status()).toContain('All changes saved'))
    expect(status()).toContain('words')
  })

  it('an agent here: the Live chip, you first in the stack with no away people, and the panel opens with Here and Away', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    expect((await screen.findByTestId('lv-state')).textContent).toBe('Live')
    const stack = screen.getByTestId('lv-stack')
    expect([...stack.querySelectorAll('[data-lv-stack-item]')].map((e) => e.getAttribute('title'))).toEqual(['You', 'Atlas (agent): follow it'])
    const panel = await screen.findByTestId('ed-live-panel')
    expect(within(panel).getByTestId('ed-live-tab-count').textContent).toBe('2')
    expect(within(panel).getByTestId('lv-watching-count').textContent).toBe('2')
    expect(within(panel).getByTestId('lv-away-count').textContent?.trim()).toBe('1')
    expect(within(panel).getByTestId('lv-person-agent:atlas').textContent).toContain('in Setup')
    expect(within(panel).getByTestId('lv-person-person:brook').textContent).toContain('not in the file now')
    expect(within(panel).getByTestId('ed-undo-tip').textContent).toContain('⌘Z undoes only your own edits.')
  })
})

describe('disconnected', () => {
  it('says so once, greys the people, strikes the count, turns Live edits off and keeps the words on this computer', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, everyone)
    await screen.findByTestId('ed-live-panel')
    t.down = true
    act(() => t.end({ reason: 'network', retry: true }))
    const bar = await screen.findByTestId('ed-save-notice')
    expect(bar.getAttribute('data-kind')).toBe('live:connection:reconnecting')
    expect(bar.textContent).toContain('Connection lost; K2 keeps trying.')
    expect(screen.getByTestId('editor-tab').hasAttribute('data-offline')).toBe(true)
    expect(screen.getByTestId('lv-state').textContent).toBe('Reconnecting')
    expect(screen.getByTestId('ed-live-tab-count').hasAttribute('data-stale')).toBe(true)
    expect(screen.getByTestId('lv-section-watching').textContent).toContain('Last seen')
    expect(screen.getByTestId('lv-person-agent:atlas').textContent).toContain('last seen in Setup')
    expect(screen.getByTestId('ed-live-edits-open').getAttribute('aria-disabled')).toBe('true')
    expect(screen.getByTestId('ed-live-edits-open').getAttribute('title')).toBe('Back online to use this')
    expect(screen.queryByTestId('lv-sentence')).toBeNull()
    expect(status()).toContain('Saved on this computer')
  })
})

describe('an outside save merged in', () => {
  it('shows the merged bar; Use their version calls the route with the save’s rev, then offers Undo', async () => {
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push({ type: 'outside_save', at_ms: 5, doc: 'd', outcome: 'ingested', rev: 41 }))
    const bar = await screen.findByTestId('ed-save-notice')
    expect(bar.textContent).toContain('Saved outside K2. Its changes are merged in.')
    await waitFor(() => expect(status()).toContain('Merged · All changes saved'))
    fireEvent.click(within(bar).getByText('Use their version'))
    await waitFor(() => expect(post).toHaveBeenCalledWith(scope, 'docs/use-theirs', { path: '/w/README.md', rev: 41 }))
    await waitFor(() => expect(screen.getByTestId('ed-save-notice').textContent).toContain('Using the version saved outside K2.'))
    expect(within(screen.getByTestId('ed-save-notice')).getByText('Undo')).toBeTruthy()
  })

  it('theirs_gone says K2 no longer has that save', async () => {
    post.mockImplementationOnce(async () => {
      throw Object.assign(new Error('gone'), { status: 409, code: 'theirs_gone' })
    })
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push({ type: 'outside_save', at_ms: 5, doc: 'd', outcome: 'ingested', rev: 41 }))
    fireEvent.click(within(await screen.findByTestId('ed-save-notice')).getByText('Use their version'))
    await waitFor(() => expect(screen.getByTestId('ed-save-notice').textContent).toContain('K2 no longer has the text of that save.'))
  })

  it('missing: Save it again writes the live text as a new file', async () => {
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push({ type: 'outside_save', at_ms: 6, doc: 'd', outcome: 'missing' }))
    const bar = await screen.findByTestId('ed-save-notice')
    expect(bar.textContent).toContain('README.md was deleted or moved outside K2.')
    await waitFor(() => expect(status()).toContain('Not saved: the file was deleted or moved'))
    fireEvent.click(within(bar).getByText('Save it again'))
    await waitFor(() => expect(post).toHaveBeenCalledWith(scope, 'fs/write-file', { path: '/w/README.md', content: DOC, expect: 'absent' }))
    await waitFor(() => expect(screen.queryByTestId('ed-save-notice')).toBeNull())
  })
})

describe('edits paused', () => {
  it('declined: opens in Read, Write off with why, and the status line never says saved', async () => {
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push({ type: 'outside_save', at_ms: 7, doc: 'd', outcome: 'declined', code: 'not_text', message: 'It isn’t UTF-8 text' }))
    expect((await screen.findByTestId('ed-save-notice')).textContent).toContain('Live edits are paused. This is the last good version.')
    await waitFor(() => expect(screen.getByTestId('editor-tab').getAttribute('data-mode')).toBe('read'))
    expect(screen.getByTestId('lv-state').textContent).toBe('Edits paused')
    expect(status()).toContain('Not saved: live edits are paused')
  })
})

describe('a refusal', () => {
  it('is a Changes row with Ask <holder>, which opens a message to them', async () => {
    const t = new ScriptedTransport()
    await open(t)
    roster(t, [...everyone, member('person:sam', 'Sam', { cursor: caret('A table.') })])
    act(() =>
      t.push({ type: 'verb_refused', at_ms: 9, actor: { id: 'agent:atlas', name: 'Atlas', kind: 'agent', colour: null }, client: 3, turn: null, verb: 'edit', unit: null, code: 'MPC-E301', holder: { id: 'person:sam', name: 'Sam', kind: 'person', colour: null }, claim: 'claim-2' }),
    )
    const row = (await screen.findAllByTestId('lv-change'))[0]
    expect(row.textContent).toContain('Atlas couldn’t edit Configuration: Sam is holding it')
    fireEvent.click(within(row).getByText('Ask Sam'))
    const ask = await screen.findByTestId('ed-ask')
    fireEvent.click(within(ask).getByText('Send'))
    await waitFor(() => expect(post).toHaveBeenCalledWith(scope, 'docs/say', { path: '/w/README.md', text: 'Could you tell me when you’re done with Configuration? I’d like to edit it.', to: 'person:sam' }))
  })
})

describe('read only and live off', () => {
  it('read only opens in Read; Write is off with who can grant access; Source stays', async () => {
    const t = new ScriptedTransport({ rights: 'read', access_owner: { id: 'person:owner', name: 'Sam', kind: 'person', colour: null } })
    await open(t)
    await waitFor(() => expect(screen.getByTestId('editor-tab').getAttribute('data-mode')).toBe('read'))
    const write = screen.getByRole('radio', { name: /Write/ })
    expect(write.getAttribute('aria-disabled')).toBe('true')
    const tip = document.getElementById(write.getAttribute('aria-describedby') ?? '')
    expect(tip?.textContent).toBe('You can view this file but not edit it. Ask Sam for edit access')
    expect(screen.getByRole('radio', { name: /Source/ }).getAttribute('aria-disabled')).toBeNull()
    expect(screen.getByTestId('ed-save-notice').textContent).toContain('Read only: you can view README.md but not edit it.')
  })

  it('the core turned off mid-session hands over (the plain editor carries on)', async () => {
    const t = new ScriptedTransport()
    const onCoreOff = vi.fn()
    await open(t, { onCoreOff })
    act(() => t.end({ reason: 'closed', retry: false, code: 'core_off' }))
    await waitFor(() => expect(onCoreOff).toHaveBeenCalledTimes(1))
    expect(screen.queryByTestId('ed-save-notice')).toBeNull()
  })
})
