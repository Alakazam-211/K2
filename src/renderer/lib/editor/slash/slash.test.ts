// @vitest-environment jsdom
// The / menu in Write mode (build-editors.md E3; editors-notes.md §2): it
// opens only when you type "/" on an empty line, filters as you keep typing,
// Esc closes it (and it stays closed on that line), moving off the line closes
// it, and picking a block puts the block's markdown in place of the "/…".
// Its keys live in the editor (a keymap of the view), so they never reach the
// Thread composer's own / menu, a different component.

import { afterEach, describe, expect, it } from 'vitest'
import { EditorSelection, EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { SLASH_BLOCKS, closeSlash, filterBlocks, insertBlockSpec, slashExtension, slashState } from './slash'

const views: EditorView[] = []
afterEach(() => {
  while (views.length) views.pop()!.destroy()
  document.body.innerHTML = ''
})

function open(doc: string, at: number): EditorView {
  const view = new EditorView({ state: EditorState.create({ doc, selection: EditorSelection.cursor(at), extensions: [slashExtension()] }), parent: document.body })
  views.push(view)
  return view
}
const type = (v: EditorView, text: string): void => {
  const at = v.state.selection.main.head
  v.dispatch({ changes: { from: at, insert: text }, selection: { anchor: at + text.length }, userEvent: 'input.type' })
}
const key = (v: EditorView, k: string): boolean => {
  const e = new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true })
  v.contentDOM.dispatchEvent(e)
  return e.defaultPrevented
}

describe('opening', () => {
  it('"/" typed on an empty line opens it', () => {
    const v = open('Intro\n\nMore', 6)
    type(v, '/')
    expect(slashState(v.state)).toMatchObject({ from: 6, query: '', active: 0 })
  })
  it('a line of only spaces (a list\'s indent) counts as empty; the block keeps the indent', () => {
    const v = open('- one\n  ', 8)
    type(v, '/')
    expect(slashState(v.state)).toMatchObject({ from: 8, query: '' })
    type(v, 'bul')
    v.dispatch(insertBlockSpec(v.state, slashState(v.state)!, 'bullet'))
    expect(v.state.doc.toString()).toBe('- one\n  - ')
  })
  it('"/" anywhere else is just a slash', () => {
    const v = open('Intro', 5)
    type(v, '/')
    expect(slashState(v.state)).toBeNull()
  })
  it('pasting "/" is not typing it', () => {
    const v = open('\n', 0)
    v.dispatch({ changes: { from: 0, insert: '/' }, userEvent: 'input.paste' })
    expect(slashState(v.state)).toBeNull()
  })
})

describe('filtering', () => {
  it('lists every block, then filters by name and other words as you type', () => {
    expect(filterBlocks('').map((b) => b.id)).toEqual(SLASH_BLOCKS.map((b) => b.id))
    expect(SLASH_BLOCKS.map((b) => b.label)).toEqual(['Heading', 'Bulleted list', 'Task list', 'Quote', 'Code block', 'Table', 'Divider', 'Image'])
    expect(filterBlocks('ta').map((b) => b.label)).toEqual(['Task list', 'Table'])
    expect(filterBlocks('todo').map((b) => b.label)).toEqual(['Task list'])
    expect(filterBlocks('HR').map((b) => b.label)).toEqual(['Divider'])
    expect(filterBlocks('zzz')).toEqual([])
  })
  it('keeps the query as you type, and the first match is picked', () => {
    const v = open('\n', 0)
    type(v, '/')
    key(v, 'ArrowDown')
    expect(slashState(v.state)?.active).toBe(1)
    type(v, 'ta')
    expect(slashState(v.state)).toMatchObject({ query: 'ta', active: 0 })
  })
  it('arrows move through the matches and wrap', () => {
    const v = open('\n', 0)
    type(v, '/')
    expect(key(v, 'ArrowUp')).toBe(true)
    expect(slashState(v.state)?.active).toBe(SLASH_BLOCKS.length - 1)
    key(v, 'ArrowDown')
    expect(slashState(v.state)?.active).toBe(0)
  })
})

describe('closing', () => {
  it('Esc closes it, keeps the "/", and typing on that line does not open it again', () => {
    const v = open('\n', 0)
    type(v, '/')
    expect(key(v, 'Escape')).toBe(true)
    expect(slashState(v.state)).toBeNull()
    type(v, 'ta')
    expect(slashState(v.state)).toBeNull()
    expect(v.state.doc.toString()).toBe('/ta\n')
  })
  it('Esc while it is closed is not taken', () => {
    const v = open('x', 1)
    expect(key(v, 'Escape')).toBe(false)
  })
  it('the caret leaving the line, or the "/" deleted, closes it', () => {
    const v = open('\nnext', 0)
    type(v, '/')
    v.dispatch({ selection: { anchor: v.state.doc.length } })
    expect(slashState(v.state)).toBeNull()
    const w = open('\n', 0)
    type(w, '/')
    w.dispatch({ changes: { from: 0, to: 1 }, userEvent: 'delete.backward' })
    expect(slashState(w.state)).toBeNull()
  })
  it('a closeSlash effect closes it', () => {
    const v = open('\n', 0)
    type(v, '/')
    v.dispatch({ effects: closeSlash.of(null) })
    expect(slashState(v.state)).toBeNull()
  })
})

describe('picking a block', () => {
  const pick = (doc: string, at: number, query: string, id: string): { text: string; caret: number } => {
    const v = open(doc, at)
    type(v, '/')
    if (query) type(v, query)
    const s = slashState(v.state)!
    v.dispatch(insertBlockSpec(v.state, s, id))
    return { text: v.state.doc.toString(), caret: v.state.selection.main.head }
  }
  it('Heading, lists, task and quote replace "/…" with their marker, the caret after it', () => {
    expect(pick('\n', 0, 'he', 'heading')).toEqual({ text: '## \n', caret: 3 })
    expect(pick('', 0, '', 'bullet')).toEqual({ text: '- ', caret: 2 })
    expect(pick('', 0, 'todo', 'task')).toEqual({ text: '- [ ] ', caret: 6 })
    expect(pick('', 0, '', 'quote')).toEqual({ text: '> ', caret: 2 })
  })
  it('a code block puts the caret inside the fence', () => {
    expect(pick('', 0, '', 'code')).toEqual({ text: '```\n\n```', caret: 4 })
  })
  it('a table is a header, a rule and a row, the caret in the first cell', () => {
    const r = pick('', 0, '', 'table')
    expect(r.text).toBe('| Column | Column |\n| --- | --- |\n|  |  |')
    expect(r.caret).toBe(r.text.lastIndexOf('|  |') + 2)
  })
  it('a divider never turns the line above into a heading: it keeps a blank line before it', () => {
    expect(pick('Words\n', 6, '', 'divider').text).toBe('Words\n\n---\n')
    expect(pick('Words\n\n', 7, '', 'divider').text).toBe('Words\n\n---\n')
  })
  it('Enter picks the chosen one', () => {
    const v = open('', 0)
    type(v, '/')
    key(v, 'ArrowDown')
    expect(key(v, 'Enter')).toBe(true)
    expect(v.state.doc.toString()).toBe('- ')
    expect(slashState(v.state)).toBeNull()
  })
  it('Enter with nothing matching is not taken (the line breaks as usual)', () => {
    const v = open('', 0)
    type(v, '/')
    type(v, 'zzz')
    expect(slashState(v.state)).not.toBeNull()
    expect(key(v, 'Enter')).toBe(false)
  })
})
