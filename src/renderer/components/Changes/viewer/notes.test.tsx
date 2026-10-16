// @vitest-environment jsdom
// C5 review notes in the C3 viewer: the gutter starts a note (one line or
// the selected lines), saved notes show as cards + gutter markers on their
// (followed) lines, outdated ones are flagged, card actions call back, and
// a new list re-places them live.
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { EditorView } from '@codemirror/view'
import { EditorSelection } from '@codemirror/state'
import { ChangesDiffView } from './ChangesDiffView'
import { placeNotes } from './notesPlacement'
import { anchorFromText, locateNote, splitLines, type ReviewNote } from '@/stores/git-notes'
import type { ChangesNotesApi } from '../diff-slot-types'
import type { ChangesDiffFile, ChangesDiffViewerProps, DiffSidesResponse } from './types'

beforeAll(() => {
  const rect = { x: 0, y: 0, top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, toJSON: () => ({}) }
  const list = Object.assign([], { item: () => null }) as unknown as DOMRectList
  Range.prototype.getBoundingClientRect = () => rect as DOMRect
  Range.prototype.getClientRects = () => list
  if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => {}
})

afterEach(() => cleanup())

const OLD = 'one\ntwo\nthree\nfour\nfive\n'
const NEW = 'one\nTWO\nthree\nfour\nfive\n'

const ROW: ChangesDiffFile = { path: 'src/a.ts', oldPath: null, group: 'unstaged', status: 'M', additions: 1, deletions: 1, binary: false, dir: false }

function sides(newText = NEW): DiffSidesResponse {
  const s = (label: string, text: string) => ({ label, exists: true, text, size: text.length, binary: false, tooLarge: false })
  return {
    path: ROW.path,
    oldPath: null,
    mode: 'unstaged',
    baseRef: null,
    rev: 'r1',
    old: s('INDEX', OLD),
    new: s('WORKTREE', newText),
    binary: false,
    tooLarge: false,
    additions: 1,
    deletions: 1,
  }
}

function note(id: string, extra: Partial<ReviewNote> = {}): ReviewNote {
  return {
    id,
    workspaceId: 'p',
    workspacePath: '/ws',
    path: ROW.path,
    side: 'new',
    startLine: 2,
    endLine: 2,
    body: `note ${id}`,
    author: 'user:ana',
    authorName: 'ana',
    createdAt: 1,
    updatedAt: 1,
    sentAt: null,
    deliveryId: null,
    resolvedAt: null,
    resolvedBy: null,
    rev: 'r1',
    blob: null,
    anchorText: 'TWO',
    outdated: false,
    currentStartLine: 2,
    currentEndLine: 2,
    ...extra,
  }
}

function api(notes: ReviewNote[], canWrite = true): ChangesNotesApi & { [k: string]: unknown } {
  return {
    notes,
    canWrite,
    add: vi.fn(async () => {}),
    update: vi.fn(async () => {}),
    resolve: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
  }
}

function props(notes: ChangesNotesApi, p: Partial<ChangesDiffViewerProps> = {}, newText = NEW): ChangesDiffViewerProps {
  return {
    workspacePath: '/ws',
    view: 'single',
    mode: 'inline',
    base: null,
    file: ROW,
    files: [ROW],
    sides: { status: 'ready', data: sides(newText) },
    rev: 'r7',
    loadSides: vi.fn(async () => sides(newText)),
    notes,
    ...p,
  }
}

function gutterCells(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll('.cm-k2-notes-gutter .cm-gutterElement')).filter(
    (el) => !(el as HTMLElement).style.visibility,
  ) as HTMLElement[]
}

function editorViews(container: HTMLElement): EditorView[] {
  return Array.from(container.querySelectorAll('.cm-editor'))
    .map((el) => EditorView.findFromDOM(el as HTMLElement))
    .filter((v): v is EditorView => v !== null)
}

describe('note anchors (same rules as the daemon)', () => {
  it('follow moved lines, flag rewritten ones, handle blank lines', () => {
    expect(splitLines('a\nb\n')).toEqual(['a', 'b'])
    expect(splitLines('a\r\nb')).toEqual(['a', 'b'])
    expect(anchorFromText('a\n\nb\n', 2, 2)).toBe('')
    expect(locateNote({ anchorText: 'b\nc', startLine: 2, endLine: 3 }, 'a\nb\nc\n')).toEqual({ start: 2, end: 3 })
    expect(locateNote({ anchorText: 'b\nc', startLine: 2, endLine: 3 }, 'x\ny\na\nb\nc\n')).toEqual({ start: 4, end: 5 })
    expect(locateNote({ anchorText: 'b\nc', startLine: 2, endLine: 3 }, 'a\nB\nc\n')).toBeNull()
    expect(locateNote({ anchorText: null, startLine: 9, endLine: 9 }, 'a\n')).toBeNull()
  })

  it('placeNotes keeps open notes of this file and side, clamps outdated ones', () => {
    const marks = placeNotes(
      [note('a'), note('b', { anchorText: 'gone', startLine: 40, endLine: 41 }), note('c', { resolvedAt: 1 }), note('d', { side: 'old' }), note('e', { path: 'x' })],
      ROW.path,
      'new',
      NEW,
    )
    expect(marks.map((m) => [m.id, m.line, m.endLine, m.outdated])).toEqual([
      ['a', 2, 2, false],
      ['b', 5, 5, true],
    ])
  })
})

describe('review notes in the diff viewer', () => {
  it('shows an open note as a card and a gutter marker; resolved notes stay out', async () => {
    const { container } = render(<ChangesDiffView {...props(api([note('n1'), note('n2', { resolvedAt: 5 })]))} />)
    await waitFor(() => expect(screen.getAllByTestId('note-card')).toHaveLength(1))
    const card = screen.getByTestId('note-card')
    expect(card.dataset.noteId).toBe('n1')
    expect(card.textContent).toContain('note n1')
    expect(card.textContent).toContain('ana · line 2')
    expect(card.hasAttribute('data-outdated')).toBe(false)
    expect(container.querySelectorAll('[data-testid="note-dot"]')).toHaveLength(1)
  })

  it('an outdated note is flagged; a moved one follows its line', async () => {
    const n = api([note('moved', { anchorText: 'three', startLine: 1, endLine: 1 }), note('gone', { anchorText: 'nope' })])
    render(<ChangesDiffView {...props(n)} />)
    await waitFor(() => expect(screen.getAllByTestId('note-card')).toHaveLength(2))
    const byId = (id: string) => screen.getAllByTestId('note-card').find((c) => c.dataset.noteId === id)!
    expect(byId('moved').textContent).toContain('line 3')
    expect(byId('moved').hasAttribute('data-outdated')).toBe(false)
    expect(byId('gone').hasAttribute('data-outdated')).toBe(true)
    expect(byId('gone').textContent).toContain('outdated')
  })

  // jsdom has no layout, so every gutter hit lands on line 1 (the browser
  // maps the pointer's height to its line); the line logic is covered by
  // the selection + `c` test below.
  it('clicking a line gutter opens the composer; Add note sends the line, its text and the rev', async () => {
    const n = api([])
    const { container } = render(<ChangesDiffView {...props(n)} />)
    await waitFor(() => expect(gutterCells(container).length).toBeGreaterThan(2))
    await act(async () => {
      fireEvent.mouseDown(gutterCells(container)[0])
    })
    const input = (await screen.findByTestId('note-input')) as HTMLTextAreaElement
    expect(screen.getByTestId('note-composer').textContent).toContain('Note on line 1')
    fireEvent.change(input, { target: { value: '  Why upper case?  ' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('note-save'))
    })
    expect(n.add).toHaveBeenCalledWith({
      filePath: 'src/a.ts',
      side: 'new',
      startLine: 1,
      endLine: 1,
      body: 'Why upper case?',
      anchorText: 'one',
      rev: 'r7',
    })
    await waitFor(() => expect(screen.queryByTestId('note-composer')).toBeNull())
  })

  it('with lines selected, `c` starts a note covering them; Esc cancels', async () => {
    const n = api([])
    const { container } = render(<ChangesDiffView {...props(n)} />)
    await waitFor(() => expect(gutterCells(container).length).toBeGreaterThan(3))
    const view = editorViews(container)[0]
    await act(async () => {
      view.dispatch({ selection: EditorSelection.range(view.state.doc.line(2).from, view.state.doc.line(4).from + 2) })
    })
    await act(async () => {
      fireEvent.keyDown(view.contentDOM, { key: 'c' })
    })
    expect(screen.getByTestId('note-composer').textContent).toContain('Note on lines 2–4')
    const input = screen.getByTestId('note-input') as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: 'all three' } })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    })
    expect(n.add).toHaveBeenCalledWith(expect.objectContaining({ startLine: 2, endLine: 4, anchorText: 'TWO\nthree\nfour' }))

    await act(async () => {
      fireEvent.mouseDown(gutterCells(container)[0])
    })
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('note-input'), { key: 'Escape' })
    })
    await waitFor(() => expect(screen.queryByTestId('note-composer')).toBeNull())
    expect(n.add).toHaveBeenCalledTimes(1)
  })

  it('card Edit / Resolve / Delete call back; a failed save keeps the composer with the error', async () => {
    const n = api([note('n1')])
    ;(n.update as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('not_author'))
    render(<ChangesDiffView {...props(n)} />)
    await waitFor(() => expect(screen.getByTestId('note-card')).toBeTruthy())
    await act(async () => {
      fireEvent.click(screen.getByTestId('note-resolve'))
    })
    expect(n.resolve).toHaveBeenCalledWith('n1')
    await act(async () => {
      fireEvent.click(screen.getByTestId('note-delete'))
    })
    expect(n.remove).toHaveBeenCalledWith('n1')
    await act(async () => {
      fireEvent.click(screen.getByTestId('note-edit'))
    })
    const input = screen.getByTestId('note-input') as HTMLTextAreaElement
    expect(input.value).toBe('note n1')
    expect(screen.queryByTestId('note-card')).toBeNull()
    fireEvent.change(input, { target: { value: 'better' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('note-save'))
    })
    expect(n.update).toHaveBeenCalledWith('n1', 'better')
    expect(screen.getByTestId('note-composer').textContent).toContain('not_author')
  })

  it('a view-only room shows notes but no gutter actions', async () => {
    const { container } = render(<ChangesDiffView {...props(api([note('n1')], false))} />)
    await waitFor(() => expect(screen.getByTestId('note-card')).toBeTruthy())
    expect(screen.queryByTestId('note-edit')).toBeNull()
    for (const cell of gutterCells(container)) {
      await act(async () => {
        fireEvent.mouseDown(cell)
      })
    }
    expect(screen.queryByTestId('note-composer')).toBeNull()
  })

  it('a new notes list re-places the cards live; split view puts old-side notes on the left', async () => {
    const first = api([note('n1')])
    const { rerender, container } = render(<ChangesDiffView {...props(first)} />)
    await waitFor(() => expect(screen.getAllByTestId('note-card')).toHaveLength(1))
    rerender(<ChangesDiffView {...props(api([note('n1'), note('n2', { anchorText: 'four', startLine: 4, endLine: 4 })]))} />)
    await waitFor(() => expect(screen.getAllByTestId('note-card')).toHaveLength(2))

    rerender(<ChangesDiffView {...props(api([note('old1', { side: 'old', anchorText: 'two' })]), { mode: 'split' })} />)
    await waitFor(() => expect(screen.getAllByTestId('note-card')).toHaveLength(1))
    const [a] = editorViews(container)
    expect(a.dom.contains(screen.getByTestId('note-card'))).toBe(true)
  })
})
