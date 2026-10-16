// @vitest-environment jsdom
// The Changes diff viewer (C3) against C2's seam props: fallbacks, inline vs
// split, next/previous change, and the lazy "view all" list.
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { EditorView } from '@codemirror/view'
import { ChangesDiffView, navKeyDirection } from './ChangesDiffView'
import type { ChangesDiffFile, ChangesDiffViewerProps, DiffSide, DiffSidesResponse } from './types'

// jsdom has no layout; CodeMirror measures text through Range rects.
beforeAll(() => {
  const rect = { x: 0, y: 0, top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, toJSON: () => ({}) }
  const list = Object.assign([], { item: () => null }) as unknown as DOMRectList
  Range.prototype.getBoundingClientRect = () => rect as DOMRect
  Range.prototype.getClientRects = () => list
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {}
})

function row(p: Partial<ChangesDiffFile> = {}): ChangesDiffFile {
  return { path: 'src/a.ts', oldPath: null, group: 'unstaged', status: 'M', additions: 2, deletions: 2, binary: false, dir: false, ...p }
}

function side(p: Partial<DiffSide> = {}): DiffSide {
  return { label: 'INDEX', exists: true, text: '', size: 0, binary: false, tooLarge: false, ...p }
}

const OLD = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten', 'eleven', 'twelve', 'thirteen', 'fourteen', 'fifteen', 'sixteen', 'seventeen', 'eighteen', 'nineteen', 'twenty'].join('\n') + '\n'
// Two separate changes: line 2 and line 18.
const NEW = OLD.replace('two\n', 'TWO\n').replace('eighteen\n', 'EIGHTEEN\n')

function sides(p: Partial<DiffSidesResponse> = {}, r: ChangesDiffFile = row()): DiffSidesResponse {
  return {
    path: r.path,
    oldPath: r.oldPath,
    mode: r.group === 'staged' ? 'staged' : r.group === 'unstaged' ? 'unstaged' : 'worktree',
    baseRef: null,
    rev: 'r1',
    old: side({ label: 'INDEX', text: OLD, size: OLD.length }),
    new: side({ label: 'WORKTREE', text: NEW, size: NEW.length }),
    binary: false,
    tooLarge: false,
    additions: 2,
    deletions: 2,
    ...p,
  }
}

function props(p: Partial<ChangesDiffViewerProps> = {}): ChangesDiffViewerProps {
  const file = p.file === undefined ? row() : p.file
  return {
    workspacePath: '/abs/ws',
    view: 'single',
    mode: 'inline',
    base: null,
    file,
    files: file ? [file] : [],
    sides: file ? { status: 'ready', data: sides({}, file) } : { status: 'idle' },
    rev: 'r1',
    loadSides: vi.fn(async (f: ChangesDiffFile) => sides({}, f)),
    ...p,
  }
}

beforeEach(() => {
  try {
    window.localStorage.clear()
  } catch {
    /* storage blocked */
  }
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('navKeyDirection', () => {
  it('maps F7 / Shift+F7 and Alt+Down / Alt+Up', () => {
    const k = (key: string, m: Partial<KeyboardEvent> = {}) =>
      navKeyDirection({ key, shiftKey: false, altKey: false, metaKey: false, ctrlKey: false, ...m })
    expect(k('F7')).toBe(1)
    expect(k('F7', { shiftKey: true })).toBe(-1)
    expect(k('ArrowDown', { altKey: true })).toBe(1)
    expect(k('ArrowUp', { altKey: true })).toBe(-1)
    expect(k('ArrowDown')).toBe(0)
    expect(k('F7', { metaKey: true })).toBe(0)
    expect(k('ArrowDown', { altKey: true, shiftKey: true })).toBe(0)
  })
})

describe('single file', () => {
  it('shows an empty state with no file', () => {
    render(<ChangesDiffView {...props({ file: null })} />)
    expect(screen.getByTestId('diff-empty').textContent).toMatch(/Select a file/)
  })

  it('renders an inline merge view with both changes, and a split view when mode is split', () => {
    const p = props()
    const { rerender, container } = render(<ChangesDiffView {...p} />)
    const ed = screen.getByTestId('merge-diff-editor')
    expect(ed.getAttribute('data-layout')).toBe('inline')
    expect(container.querySelector('.cm-mergeView')).toBeNull()
    expect(container.querySelectorAll('.cm-deletedChunk').length).toBe(2)
    expect(screen.getByTestId('diff-counter').textContent).toBe('0 / 2')

    rerender(<ChangesDiffView {...p} mode="split" />)
    expect(screen.getByTestId('merge-diff-editor').getAttribute('data-layout')).toBe('split')
    const mv = container.querySelector('.cm-mergeView')
    expect(mv).not.toBeNull()
    expect(mv!.querySelectorAll('.cm-editor').length).toBe(2)
    expect(screen.getByTestId('diff-counter').textContent).toBe('0 / 2')
  })

  it('F7 / Shift+F7 walk the changes and wrap', () => {
    render(<ChangesDiffView {...props()} />)
    const pane = screen.getByTestId('changes-diff-single')
    const counter = screen.getByTestId('diff-counter')
    fireEvent.keyDown(pane, { key: 'F7' })
    expect(counter.textContent).toBe('1 / 2')
    fireEvent.keyDown(pane, { key: 'F7' })
    expect(counter.textContent).toBe('2 / 2')
    fireEvent.keyDown(pane, { key: 'F7' })
    expect(counter.textContent).toBe('1 / 2')
    fireEvent.keyDown(pane, { key: 'F7', shiftKey: true })
    expect(counter.textContent).toBe('2 / 2')
    fireEvent.click(screen.getByTestId('diff-prev'))
    expect(counter.textContent).toBe('1 / 2')
    fireEvent.click(screen.getByTestId('diff-next'))
    expect(counter.textContent).toBe('2 / 2')
  })

  it('the cursor lands on the changed line in the new text', () => {
    const { container } = render(<ChangesDiffView {...props()} />)
    fireEvent.keyDown(screen.getByTestId('changes-diff-single'), { key: 'F7' })
    const view = EditorView.findFromDOM(container.querySelector('.cm-editor') as HTMLElement)
    expect(view).not.toBeNull()
    // The selection is at the start of "TWO" (line 2 of the new text).
    expect(view!.state.doc.lineAt(view!.state.selection.main.head).number).toBe(2)
    expect(view!.state.readOnly).toBe(true)
  })

  it('binary → a "Binary file" card with sizes, no editor', () => {
    const r = row({ binary: true, additions: null, deletions: null })
    render(
      <ChangesDiffView
        {...props({
          file: r,
          sides: {
            status: 'ready',
            data: sides({ binary: true, old: side({ text: null, binary: true, size: 2048 }), new: side({ text: null, binary: true, size: 4096 }), additions: null, deletions: null }, r),
          },
        })}
      />,
    )
    expect(screen.getByTestId('diff-binary').textContent).toMatch(/Binary file.*2\.0 KB → 4\.0 KB/)
    expect(screen.queryByTestId('merge-diff-editor')).toBeNull()
    expect(screen.getByTestId('diff-counter').textContent).toBe('No changes')
  })

  it('too large → message + "Open in editor" (only when C2 passes onOpenFile)', () => {
    const r = row()
    const big = sides({ tooLarge: true, new: side({ text: null, tooLarge: true, size: 11 * 1024 * 1024 }), additions: null, deletions: null }, r)
    const { rerender } = render(<ChangesDiffView {...props({ file: r, sides: { status: 'ready', data: big } })} />)
    const card = screen.getByTestId('diff-too-large')
    expect(card.textContent).toMatch(/too large/)
    expect(within(card).queryByText('Open in editor')).toBeNull()

    const onOpenFile = vi.fn()
    rerender(<ChangesDiffView {...props({ file: r, sides: { status: 'ready', data: big }, onOpenFile })} />)
    fireEvent.click(within(screen.getByTestId('diff-too-large')).getByText('Open in editor'))
    expect(onOpenFile).toHaveBeenCalledWith('src/a.ts')
  })

  it('an added file diffs against empty; a deleted file against an empty new side', () => {
    const added = row({ group: 'untracked', status: 'U' })
    const { container, rerender } = render(
      <ChangesDiffView
        {...props({ file: added, sides: { status: 'ready', data: sides({ old: side({ exists: false, text: '', size: 0 }) }, added) } })}
      />,
    )
    expect(screen.getByTestId('merge-diff-editor')).toBeTruthy()
    // Nothing old to show: no deleted lines, every new line marked.
    expect(container.querySelectorAll('.cm-deletedLine').length).toBe(0)
    expect(container.querySelectorAll('.cm-changedLine').length).toBe(20)

    const deleted = row({ status: 'D' })
    rerender(
      <ChangesDiffView
        {...props({ file: deleted, sides: { status: 'ready', data: sides({ new: side({ exists: false, text: '', size: 0 }) }, deleted) } })}
      />,
    )
    // Every old line shows as deleted above an empty new doc.
    expect(container.querySelectorAll('.cm-deletedLine').length).toBe(20)
  })

  it('a rename shows old → new in the header', () => {
    const r = row({ status: 'R', oldPath: 'src/old.ts' })
    render(<ChangesDiffView {...props({ file: r, sides: { status: 'ready', data: sides({ oldPath: 'src/old.ts' }, r) } })} />)
    expect(screen.getByTestId('diff-file-title').textContent).toBe('src/old.ts → src/a.ts')
  })

  it('an untracked folder row shows a folder card', () => {
    const r = row({ path: 'newdir/', dir: true, group: 'untracked', status: 'U', additions: null, deletions: null })
    render(<ChangesDiffView {...props({ file: r, sides: { status: 'idle' } })} />)
    expect(screen.getByTestId('diff-dir')).toBeTruthy()
  })

  it('loading, refresh and error states follow C2\'s sides', () => {
    const r = row()
    const { rerender } = render(<ChangesDiffView {...props({ file: r, sides: { status: 'loading' } })} />)
    expect(screen.getByTestId('diff-loading')).toBeTruthy()
    rerender(<ChangesDiffView {...props({ file: r, sides: { status: 'loading', data: sides({}, r) } })} />)
    expect(screen.getByTestId('merge-diff-editor')).toBeTruthy()
    expect(screen.getByText('updating…')).toBeTruthy()
    rerender(<ChangesDiffView {...props({ file: r, sides: { status: 'error', error: 'not_found: gone' } })} />)
    expect(screen.getByTestId('diff-error').textContent).toMatch(/not_found: gone/)
  })

  it('word wrap and collapse toggles persist per viewer', () => {
    const { container, unmount } = render(<ChangesDiffView {...props()} />)
    expect(container.querySelector('.cm-lineWrapping')).toBeNull()
    expect(container.querySelectorAll('.cm-collapsedLines').length).toBeGreaterThan(0)
    fireEvent.click(screen.getByTestId('diff-wrap'))
    expect(container.querySelector('.cm-lineWrapping')).not.toBeNull()
    fireEvent.click(screen.getByTestId('diff-collapse'))
    expect(container.querySelectorAll('.cm-collapsedLines').length).toBe(0)
    unmount()
    const again = render(<ChangesDiffView {...props()} />)
    expect(again.container.querySelector('.cm-lineWrapping')).not.toBeNull()
    expect(screen.getByTestId('diff-wrap').getAttribute('aria-pressed')).toBe('true')
  })
})

// ── View all ────────────────────────────────────────────────────────────

class FakeIO {
  static all: FakeIO[] = []
  els: Element[] = []
  constructor(public cb: IntersectionObserverCallback) {
    FakeIO.all.push(this)
  }
  observe(el: Element): void {
    this.els.push(el)
  }
  disconnect(): void {
    this.els = []
  }
  unobserve(): void {}
  takeRecords(): IntersectionObserverEntry[] {
    return []
  }
  static show(key: string): void {
    for (const io of FakeIO.all) {
      for (const el of io.els) {
        if (el.getAttribute('data-key') === key) {
          io.cb([{ isIntersecting: true, target: el } as unknown as IntersectionObserverEntry], io as unknown as IntersectionObserver)
        }
      }
    }
  }
}

describe('view all', () => {
  const a = row({ path: 'a.ts' })
  const b = row({ path: 'b.ts', group: 'staged' })
  const c = row({ path: 'c.bin', binary: true, additions: null, deletions: null })
  const files = [a, b, c]

  function loader() {
    return vi.fn(async (f: ChangesDiffFile) =>
      f.binary
        ? sides({ binary: true, old: side({ text: null, binary: true, size: 1 }), new: side({ text: null, binary: true, size: 2 }), additions: null, deletions: null }, f)
        : sides({}, f),
    )
  }

  beforeEach(() => {
    FakeIO.all = []
    vi.stubGlobal('IntersectionObserver', FakeIO)
  })

  it('stacks a sticky header per file and loads a file only once it scrolls near view', async () => {
    const loadSides = loader()
    render(<ChangesDiffView {...props({ view: 'all', file: null, files, sides: { status: 'idle' }, loadSides })} />)
    const sections = screen.getAllByTestId('diff-section')
    expect(sections.map((s) => s.getAttribute('data-key'))).toEqual(['unstaged:a.ts', 'staged:b.ts', 'unstaged:c.bin'])
    expect(sections[0].querySelector('header')!.className).toMatch(/sticky/)
    expect(loadSides).not.toHaveBeenCalled()
    expect(screen.getAllByTestId('diff-placeholder').length).toBe(3)

    await act(async () => FakeIO.show('staged:b.ts'))
    expect(loadSides).toHaveBeenCalledTimes(1)
    expect(loadSides.mock.calls[0][0]).toBe(b)
    await waitFor(() => expect(within(sections[1]).getByTestId('merge-diff-editor')).toBeTruthy())
    expect(screen.getAllByTestId('diff-placeholder').length).toBe(2)
  })

  it('a new rev reloads only the files already loaded', async () => {
    const loadSides = loader()
    const p = props({ view: 'all', file: null, files, sides: { status: 'idle' }, loadSides })
    const { rerender } = render(<ChangesDiffView {...p} />)
    await act(async () => FakeIO.show('unstaged:a.ts'))
    await waitFor(() => expect(loadSides).toHaveBeenCalledTimes(1))
    rerender(<ChangesDiffView {...p} rev="r2" />)
    await waitFor(() => expect(loadSides).toHaveBeenCalledTimes(2))
    expect(loadSides.mock.calls.map((c) => c[0].path)).toEqual(['a.ts', 'a.ts'])
  })

  it('F7 walks across files, loading the next file when it reaches it', async () => {
    const loadSides = loader()
    render(<ChangesDiffView {...props({ view: 'all', file: null, files, sides: { status: 'idle' }, loadSides })} />)
    await act(async () => FakeIO.show('unstaged:a.ts'))
    await waitFor(() => expect(screen.getByTestId('diff-counter').textContent).toBe('0 / 2+'))
    const pane = screen.getByTestId('changes-diff-all')
    fireEvent.keyDown(pane, { key: 'F7' })
    fireEvent.keyDown(pane, { key: 'F7' })
    expect(screen.getByTestId('diff-counter').textContent).toBe('2 / 2+')
    // Next: b.ts is not loaded yet — the press reveals (and loads) it.
    await act(async () => {
      fireEvent.keyDown(pane, { key: 'F7' })
    })
    await waitFor(() => expect(loadSides).toHaveBeenCalledTimes(2))
    expect(loadSides.mock.calls[1][0]).toBe(b)
    await waitFor(() => expect(screen.getByTestId('diff-counter').textContent).toBe('2 / 4+'))
    fireEvent.keyDown(pane, { key: 'F7' })
    expect(screen.getByTestId('diff-counter').textContent).toBe('3 / 4+')
  })

  it('selecting a file in the list loads it; a header click reports it', async () => {
    const loadSides = loader()
    const onSelectFile = vi.fn()
    const p = props({ view: 'all', file: null, files, sides: { status: 'idle' }, loadSides, onSelectFile })
    const { rerender } = render(<ChangesDiffView {...p} />)
    rerender(<ChangesDiffView {...p} file={c} />)
    await waitFor(() => expect(loadSides).toHaveBeenCalledTimes(1))
    expect(loadSides.mock.calls[0][0]).toBe(c)
    await waitFor(() => expect(screen.getByTestId('diff-binary')).toBeTruthy())
    fireEvent.click(screen.getAllByTestId('diff-section')[0].querySelector('header')!)
    expect(onSelectFile).toHaveBeenCalledWith(a)
  })

  it('a failed load shows the error with Retry', async () => {
    let fail = true
    const loadSides = vi.fn(async (f: ChangesDiffFile) => {
      if (fail) throw new Error('daemon down')
      return sides({}, f)
    })
    render(<ChangesDiffView {...props({ view: 'all', file: null, files: [a], sides: { status: 'idle' }, loadSides })} />)
    await act(async () => FakeIO.show('unstaged:a.ts'))
    await waitFor(() => expect(screen.getByTestId('diff-error').textContent).toMatch(/daemon down/))
    fail = false
    await act(async () => fireEvent.click(screen.getByText('Retry')))
    await waitFor(() => expect(screen.getByTestId('merge-diff-editor')).toBeTruthy())
  })

  it('an answer that lands after the rev moved on is dropped', async () => {
    const resolvers: Array<(v: DiffSidesResponse) => void> = []
    const loadSides = vi.fn(() => new Promise<DiffSidesResponse>((res) => resolvers.push(res)))
    const p = props({ view: 'all', file: null, files: [a], sides: { status: 'idle' }, loadSides })
    const { rerender, container } = render(<ChangesDiffView {...p} />)
    await act(async () => FakeIO.show('unstaged:a.ts'))
    rerender(<ChangesDiffView {...p} rev="r2" />)
    await waitFor(() => expect(resolvers.length).toBe(2))
    // The r2 answer (identical texts → "No text changes") lands first…
    await act(async () => resolvers[1](sides({ old: side({ text: 'same' }), new: side({ text: 'same' }) }, a)))
    expect(screen.getByTestId('diff-identical')).toBeTruthy()
    // …then the stale r1 answer: ignored.
    await act(async () => resolvers[0](sides({}, a)))
    expect(screen.getByTestId('diff-identical')).toBeTruthy()
    expect(container.querySelector('.cm-editor')).toBeNull()
  })

  it('no files → "No changes"', () => {
    render(<ChangesDiffView {...props({ view: 'all', file: null, files: [], sides: { status: 'idle' } })} />)
    expect(screen.getByTestId('diff-empty').textContent).toBe('No changes')
  })
})
