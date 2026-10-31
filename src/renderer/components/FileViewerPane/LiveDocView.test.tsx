// @vitest-environment jsdom

// The Live view (0.46.7): the kit's Live editor mounted over K2's transport.
// A scripted transport stands in for `WS /cli/docs/live`: the welcome, the
// roster, someone else's change, the removed-text note, and the closes.
// Pinned here:
//   - the editor goes live on the welcome and shows the doc's text;
//   - the status line counts who else is here (the roster);
//   - someone else's change shows in the text;
//   - this person's typing goes out as an `edit` frame;
//   - a removed-text note (`overwritten`, to this person) shows with Copy
//     and Dismiss, and is delivered back (`take_notes`) once shown;
//   - a note to someone else is never shown;
//   - closed with `core_off`: the tab is handed back (`onCoreOff`);
//   - closed for another reason (idle): says why, and Reopen connects again.

import '../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { EditorView } from '@codemirror/view'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { LiveDocView, closedText, liveStatusText, noteText } from './LiveDocView'
import type { LiveClientFrame, LiveClose, LiveServerFrame, LiveTransport, LiveWelcome } from '@/lib/docs-live-transport'
import type { ServerScope } from '@/kessel/server-scope'

const SELF = 'person:owner'

/** A scripted `LiveTransport`: each connect is a new connection (client
 *  id), every frame sent is kept, and the test pushes frames and closes. */
class ScriptedTransport implements LiveTransport {
  connects = 0
  sent: LiveClientFrame[] = []
  closedByEditor = false
  private frameCbs = new Set<(f: LiveServerFrame) => void>()
  private closeCbs = new Set<(e: LiveClose) => void>()
  private isOpen = false
  constructor(private text: string) {}
  get open(): boolean {
    return this.isOpen
  }
  async connect(): Promise<LiveWelcome> {
    this.connects += 1
    this.isOpen = true
    return { root: 'source', version: 1, text: this.text, client: 100 + this.connects, self: SELF, rights: 'edit' }
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
    this.closedByEditor = true
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

const scope = { connectionKey: 'test' } as unknown as ServerScope

function member(handle: string, name: string): Record<string, unknown> {
  return { handle, name, colour: '#7c3aed', kind: 'person', role: 'editor', away: false, cursor: null, rights: 'edit' }
}

function note(to: string, change: number, yours: string): Record<string, unknown> {
  return {
    kind: 'overwritten',
    to,
    change,
    version: 4,
    restore_from: 4,
    source: 'an outside save',
    rule: 'removed_text',
    row: null,
    line: 2,
    cells: [{ region: 'L2', col: null, yours, file: '', cut: false }],
    more: 0,
    not_shown: 0,
    withheld: false,
    no_history: false,
    message: `An outside save may have removed your text (L2). Version 4 holds it. It read "${yours.replace('\n', '\\n')}".`,
  }
}

function editorText(): string {
  const el = document.querySelector('.cm-editor') as HTMLElement | null
  if (!el) throw new Error('no editor mounted')
  const view = EditorView.findFromDOM(el)
  if (!view) throw new Error('no EditorView')
  return view.state.doc.toString()
}

function editorView(): EditorView {
  const view = EditorView.findFromDOM(document.querySelector('.cm-editor') as HTMLElement)
  if (!view) throw new Error('no EditorView')
  return view
}

afterEach(() => cleanup())

describe('LiveDocView', () => {
  it('goes live, counts who is here, shows their change and sends this person’s typing', async () => {
    const t = new ScriptedTransport('hello\n')
    render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown createTransport={() => t} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    expect(screen.getByTestId('live-doc-status').textContent).toBe('Live · only you here')
    expect(editorText()).toBe('hello\n')

    act(() => t.push({ type: 'roster_event', event: { kind: 'snapshot', version: 1, members: [member(SELF, 'Me'), member('person:ana', 'Ana')], watching: 0 } }))
    await waitFor(() => expect(screen.getByTestId('live-doc-status').textContent).toBe('Live · 1 other here'))

    act(() =>
      t.push({
        type: 'change',
        event: { version: 2, pid: 'person:ana', client: 7, via: 'tab', root: 'source', hunks: [{ start: 6, end: 6, insert: 'from ana\n' }], at_ms: 1 },
      }),
    )
    await waitFor(() => expect(editorText()).toBe('hello\nfrom ana\n'))

    act(() => editorView().dispatch({ changes: { from: 0, insert: '# ' }, userEvent: 'input.type' }))
    await waitFor(() => expect(t.sent.some((f) => 'type' in f && f.type === 'edit')).toBe(true))
    const edit = t.sent.find((f) => 'type' in f && f.type === 'edit') as Extract<LiveClientFrame, { type: 'edit' }>
    expect(edit.root).toBe('source')
    expect(edit.hunks).toEqual([{ start: 0, end: 0, insert: '# ' }])
  })

  it('shows a removed-text note with Copy and Dismiss, delivers it, and never shows someone else’s', async () => {
    const t = new ScriptedTransport('hello\n')
    const writeText = vi.fn(async () => undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown createTransport={() => t} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))

    act(() => t.push({ type: 'roster_event', event: note('person:ana', 9, "ana's line\n") }))
    act(() => t.push({ type: 'roster_event', event: note(SELF, 8, 'my line\n') }))
    const notes = await screen.findByTestId('live-doc-notes')
    expect(notes.textContent).toContain('An outside save may have removed your text (L2)')
    expect(notes.textContent).toContain('my line')
    expect(notes.textContent).not.toContain("ana's line")
    await waitFor(() => expect(t.sent.filter((f) => 'type' in f && f.type === 'take_notes')).toHaveLength(1))

    fireEvent.click(screen.getByText('Copy my text'))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('my line\n'))
    expect(await screen.findByText('Copied')).toBeTruthy()
    fireEvent.click(screen.getByText('Dismiss'))
    expect(screen.queryByTestId('live-doc-notes')).toBeNull()
  })

  it('hands the tab back when the core is turned off', async () => {
    const t = new ScriptedTransport('hello\n')
    const onCoreOff = vi.fn()
    render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown onCoreOff={onCoreOff} createTransport={() => t} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    act(() => t.end({ reason: 'closed', retry: false, code: 'core_off' }))
    await waitFor(() => expect(onCoreOff).toHaveBeenCalledWith({ reason: 'closed', code: 'core_off' }))
    expect(screen.getByTestId('live-doc-status').textContent).toBe(closedText({ reason: 'closed', code: 'core_off' }))
    expect(screen.queryByText('Reopen')).toBeNull()
  })

  it('says why it stopped on another close, and Reopen connects again', async () => {
    const t = new ScriptedTransport('hello\n')
    render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown={false} createTransport={() => t} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    act(() => t.end({ reason: 'idle', retry: false }))
    await waitFor(() => expect(screen.getByTestId('live-doc-status').textContent).toBe('Live editing paused: this tab was idle.'))
    expect(t.connects).toBe(1)
    fireEvent.click(screen.getByText('Reopen'))
    await waitFor(() => expect(t.connects).toBe(2))
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
  })
})

describe('the Live view’s words', () => {
  it('names the state and who is here', () => {
    expect(liveStatusText('live', 0)).toBe('Live · only you here')
    expect(liveStatusText('live', 2)).toBe('Live · 2 others here')
    expect(liveStatusText('reconnecting', 0)).toContain('Reconnecting')
  })
  it('gives back only this person’s own removed text, every place', () => {
    const n = note(SELF, 1, 'a\n') as unknown as Parameters<typeof noteText>[0]
    expect(noteText({ ...n, cells: [...n.cells, { region: 'L9', col: null, yours: 'b\n', file: '', cut: false }] })).toBe('a\nb\n')
  })
  it('says a moved file’s new name', () => {
    expect(closedText({ reason: 'moved', path: 'docs/b.md' })).toBe('This file was renamed to docs/b.md. Open it there.')
  })
})
