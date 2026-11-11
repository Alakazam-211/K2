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
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { EditorView } from '@codemirror/view'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { LiveDocView, closedText, liveStatusText, noteText, undoChord } from './LiveDocView'
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

/** A socket that answers undo the way the daemon does: the change first (as
 *  a `change` frame, never filtered: it isn't an edit of this socket's), then
 *  `undone`. `steps: 0` is nothing to undo. */
class UndoTransport extends ScriptedTransport {
  version = 1
  steps = 1
  throwOnUndo = false
  onUndoChange: (() => void) | null = null
  undos(): number {
    return this.sent.filter((f) => 'type' in f && f.type === 'undo').length
  }
  send(f: LiveClientFrame): void {
    if ('type' in f && f.type === 'undo' && this.throwOnUndo) throw new Error('socket went away')
    super.send(f)
    if (!('type' in f) || f.type !== 'undo') return
    const op = f.op
    const steps = this.steps
    queueMicrotask(() => {
      if (steps > 0) {
        this.version += 1
        this.push({
          type: 'change',
          event: { version: this.version, pid: SELF, client: 101, via: 'undo', root: 'source', hunks: [{ start: 0, end: 1, insert: '' }], at_ms: 1 },
        })
      }
      this.push({ type: 'undone', op, steps, version: this.version })
      this.onUndoChange?.()
    })
  }
}

describe('LiveDocView: the undo keys stay in the editor', () => {
  const content = (): HTMLElement => document.querySelector('.cm-content') as HTMLElement
  /** The key lands where the browser would send it: the focused element. */
  async function cmdZ(shift = false): Promise<KeyboardEvent> {
    const ev = new KeyboardEvent('keydown', { key: 'z', code: 'KeyZ', keyCode: 90, ctrlKey: true, shiftKey: shift, bubbles: true, cancelable: true })
    await act(async () => {
      ;(document.activeElement ?? document.body).dispatchEvent(ev)
      await new Promise((r) => setTimeout(r, 5))
    })
    return ev
  }
  async function open(t: ScriptedTransport): Promise<EditorView> {
    render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown createTransport={() => t} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    const view = editorView()
    view.focus()
    expect(document.activeElement).toBe(content())
    return view
  }
  // Anything outside the editor listening for the key (the page's own
  // handlers, and through them the app menu's native undo).
  let leaked: KeyboardEvent[] = []
  const onWindowKey = (e: KeyboardEvent): void => void leaked.push(e)
  beforeEach(() => {
    leaked = []
    window.addEventListener('keydown', onWindowKey)
  })
  afterEach(() => {
    window.removeEventListener('keydown', onWindowKey)
    vi.restoreAllMocks()
  })

  it('one undo, the change from that undo, then a second Cmd+Z: both reach the daemon on one editor', async () => {
    const t = new UndoTransport('hello\n')
    const view = await open(t)
    const first = await cmdZ()
    expect(first.defaultPrevented).toBe(true)
    expect(t.undos()).toBe(1)
    await waitFor(() => expect(editorText()).toBe('ello\n'))
    expect(document.activeElement).toBe(content())

    const second = await cmdZ()
    expect(second.defaultPrevented).toBe(true)
    expect(t.undos()).toBe(2)
    await waitFor(() => expect(editorText()).toBe('llo\n'))
    expect(document.activeElement).toBe(content())
    expect(editorView()).toBe(view)
    expect(t.connects).toBe(1)
    expect(leaked).toHaveLength(0)
  })

  it('nothing to undo: the key is still the editor’s and never reaches the page', async () => {
    const t = new UndoTransport('hello\n')
    t.steps = 0
    await open(t)
    const ev = await cmdZ()
    expect(ev.defaultPrevented).toBe(true)
    expect(leaked).toHaveLength(0)
    expect(document.activeElement).toBe(content())
    expect(editorText()).toBe('hello\n')
    const redo = await cmdZ(true)
    expect(redo.defaultPrevented).toBe(true)
    expect(leaked).toHaveLength(0)
  })

  it('an undo that throws is consumed and logged, and the next press still works', async () => {
    const t = new UndoTransport('hello\n')
    await open(t)
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    t.throwOnUndo = true
    const ev = await cmdZ()
    expect(ev.defaultPrevented).toBe(true)
    expect(leaked).toHaveLength(0)
    expect(errors.mock.calls.some((c) => String(c[0]).includes('undo failed'))).toBe(true)
    errors.mockRestore()
    expect(document.activeElement).toBe(content())

    t.throwOnUndo = false
    await cmdZ()
    expect(t.undos()).toBe(1)
  })

  it('a focus that falls to nothing after the undo’s change comes back to the editor', async () => {
    const t = new UndoTransport('hello\n')
    await open(t)
    // jsdom says a document with nothing focused has no focus; a real window
    // still does.
    vi.spyOn(document, 'hasFocus').mockReturnValue(true)
    // What Rosson sees: the caret drops out right after the undo's change.
    t.onUndoChange = () => content().blur()
    await cmdZ()
    await waitFor(() => expect(document.activeElement).toBe(content()))
    t.onUndoChange = null
    await cmdZ()
    expect(t.undos()).toBe(2)
    expect(leaked).toHaveLength(0)
  })

  it('a click elsewhere keeps its focus; clicking back, the first Cmd+Z is the editor’s', async () => {
    const t = new UndoTransport('hello\n')
    const view = await open(t)
    const box = document.body.appendChild(document.createElement('div'))
    vi.spyOn(document, 'hasFocus').mockReturnValue(true)
    box.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    content().blur()
    await new Promise((r) => setTimeout(r, 20))
    expect(document.activeElement).toBe(document.body)
    box.remove()

    content().dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    view.focus()
    const ev = await cmdZ()
    expect(ev.defaultPrevented).toBe(true)
    expect(t.undos()).toBe(1)
    expect(document.activeElement).toBe(content())
    expect(leaked).toHaveLength(0)
  })

  it('an editor replaced while it holds the caret: the new one has it, and Cmd+Z goes to the new one', async () => {
    const a = new UndoTransport('hello\n')
    const b = new UndoTransport('hello\n')
    const { rerender } = render(<LiveDocView scope={scope} filePath="/w/notes.md" markdown createTransport={() => a} />)
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    editorView().focus()
    const old = editorView()

    rerender(<LiveDocView scope={scope} filePath="/w/notes.md" markdown createTransport={() => b} />)
    await waitFor(() => expect(b.connects).toBe(1))
    await waitFor(() => expect(screen.getByTestId('live-doc-view').dataset.state).toBe('live'))
    expect(a.closedByEditor).toBe(true)
    expect(editorView()).not.toBe(old)
    await waitFor(() => expect(document.activeElement).toBe(content()))

    const ev = await cmdZ()
    expect(ev.defaultPrevented).toBe(true)
    expect(b.undos()).toBe(1)
    expect(a.undos()).toBe(0)
    expect(leaked).toHaveLength(0)
  })
})

describe('undoChord', () => {
  const key = (over: Partial<KeyboardEvent>) => ({ key: 'z', code: 'KeyZ', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...over }) as KeyboardEvent
  it('is Cmd on a Mac and Ctrl elsewhere', () => {
    expect(undoChord(key({ metaKey: true }), true)).toBe('undo')
    expect(undoChord(key({ metaKey: true, shiftKey: true }), true)).toBe('redo')
    expect(undoChord(key({ ctrlKey: true }), true)).toBeNull()
    expect(undoChord(key({ ctrlKey: true }), false)).toBe('undo')
    expect(undoChord(key({ metaKey: true }), false)).toBeNull()
    expect(undoChord(key({ key: 'y', code: 'KeyY', ctrlKey: true }), false)).toBe('redo')
  })
  it('leaves everything else alone', () => {
    expect(undoChord(key({}), true)).toBeNull()
    expect(undoChord(key({ metaKey: true, altKey: true }), true)).toBeNull()
    expect(undoChord(key({ key: 'a', code: 'KeyA', metaKey: true }), true)).toBeNull()
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
