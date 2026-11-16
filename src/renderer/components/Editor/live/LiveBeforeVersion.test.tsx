// @vitest-environment jsdom

// An outside save's earlier text from its stored version (K2: every
// outside_save carries `before_version`; a tab that opens after the save gets
// the frame replayed after its welcome). Pinned here:
//   - the replay alone (no external change seen here) gives the merged notice
//     with Compare and Undo their save;
//   - Compare fetches `docs/show?version=<before_version>` (quietly loading),
//     and shows the two side by side;
//   - a failed fetch: "K2 couldn't load the text from before that save." with
//     Retry, which fetches again;
//   - Undo their save puts the fetched text back as one edit of yours;
//   - when that fetch fails, Undo their save says K2 no longer has that text;
//   - with no before_version (and no text seen here) there is nothing to
//     compare or put back: neither action shows.

import '../../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const get = vi.hoisted(() => vi.fn(async (_s: unknown, _route: string, _p?: unknown): Promise<unknown> => ({})))
const post = vi.hoisted(() => vi.fn(async (_s: unknown, route: string, _b?: unknown): Promise<unknown> => ({ ok: true, route })))
vi.mock('@/lib/daemon-cli', async (orig) => ({ ...(await orig<typeof import('@/lib/daemon-cli')>()), daemonCliPost: post, daemonCliGet: get }))
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
import { EditorView } from '@codemirror/view'
const MERGED = { type: 'outside_save', at_ms: 5, doc: 'd', outcome: 'ingested', rev: 41, before_version: 40 }
const EARLIER = DOC.replace('Clone it.', 'Clone it now.')
const notice = (): HTMLElement => screen.getByTestId('ed-save-notice')

beforeEach(() => {
  post.mockClear()
  get.mockReset()
  window.localStorage.clear()
})
afterEach(() => cleanup())

describe('the earlier text from before_version', () => {
  it('the replay alone gives the merged notice; Compare fetches the version, loading quietly', async () => {
    let answer: (v: unknown) => void = () => undefined
    get.mockImplementation(() => new Promise((r) => (answer = r)))
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push(MERGED))
    const bar = await screen.findByTestId('ed-save-notice')
    expect(bar.textContent).toContain('Saved outside K2. Its changes are merged in.')
    expect(within(bar).getByText('Undo their save')).toBeTruthy()
    fireEvent.click(within(bar).getByText('Compare'))
    expect(get).toHaveBeenCalledWith(scope, 'docs/show', { path: '/w/README.md', version: 40 })
    expect(await screen.findByTestId('ed-compare-loading')).toBeTruthy()
    await act(async () => answer({ ok: true, version: 40, text: EARLIER }))
    await waitFor(() => expect(screen.queryByTestId('ed-compare-loading')).toBeNull())
    expect(await screen.findByTestId('ed-compare')).toBeTruthy()
  })

  it('a failed fetch says K2 couldn’t load the earlier text, and Retry fetches again', async () => {
    get.mockRejectedValueOnce(Object.assign(new Error('history is not shared with you'), { status: 403 }))
    get.mockResolvedValueOnce({ ok: true, version: 40, text: EARLIER })
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push(MERGED))
    fireEvent.click(within(await screen.findByTestId('ed-save-notice')).getByText('Compare'))
    await waitFor(() => expect(notice().textContent).toContain('K2 couldn’t load the text from before that save.'))
    fireEvent.click(within(notice()).getByText('Retry'))
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2))
    expect(await screen.findByTestId('ed-compare')).toBeTruthy()
  })

  it('Undo their save puts the fetched text back as one edit of yours', async () => {
    get.mockResolvedValue({ ok: true, version: 40, text: EARLIER })
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push(MERGED))
    fireEvent.click(within(await screen.findByTestId('ed-save-notice')).getByText('Undo their save'))
    fireEvent.click(within(await screen.findByRole('alert')).getByText('Undo their save'))
    await waitFor(() => expect(notice().textContent).toContain('Put back the text from before that save.'))
    const view = EditorView.findFromDOM(document.querySelector('.cm-content') as HTMLElement)
    expect(view?.state.doc.toString()).toBe(EARLIER)
    expect(t.sent.some((f) => (f as { type?: string }).type === 'edit')).toBe(true)
  })

  it('Undo their save when the earlier text can’t be fetched: K2 no longer has it', async () => {
    get.mockRejectedValue(Object.assign(new Error('version 40 is gone'), { status: 404 }))
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push(MERGED))
    fireEvent.click(within(await screen.findByTestId('ed-save-notice')).getByText('Undo their save'))
    fireEvent.click(within(await screen.findByRole('alert')).getByText('Undo their save'))
    await waitFor(() => expect(notice().textContent).toContain('K2 no longer has the text from before that save.'))
    expect(EditorView.findFromDOM(document.querySelector('.cm-content') as HTMLElement)?.state.doc.toString()).toBe(DOC)
  })

  it('no before_version and no text seen here: no Compare or Undo their save', async () => {
    const t = new ScriptedTransport()
    await open(t)
    act(() => t.push({ ...MERGED, before_version: null }))
    const bar = await screen.findByTestId('ed-save-notice')
    expect(within(bar).queryByText('Compare')).toBeNull()
    expect(within(bar).queryByText('Undo their save')).toBeNull()
    expect(get).not.toHaveBeenCalled()
  })
})
