// @vitest-environment jsdom
// The editors' E3 tools on a real CodeMirror in the frame (build-editors.md
// E3): the docked find bar (⌘F, the count, Previous and Next, Aa W .*,
// Replace and Replace all, off with a reason where it can't act), go to line
// (⌘L and the status line's Ln, Col), the outline in the side panel (click to
// jump), the formatting toolbar over a selection in Write, and the / menu.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import '../../../../../vendor/k2-mpc-base/client/test/jsdom-shims'
import { useEffect, useRef } from 'react'
import { EditorSelection, EditorState } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { EditorTab, type EditorTabProps, type EditorMountOptions } from '../EditorTab'

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const README = ['# Quill', '', 'Quill writes a draft, then a second draft.', '', '## Setup', '', 'Open the draft.', '', '## Configuration', '', 'Last line.'].join('\n')

function RealEditor({ o, text }: { o: EditorMountOptions; text: string }): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const view = new EditorView({
      state: EditorState.create({ doc: text, extensions: [history(), keymap.of([...historyKeymap, ...defaultKeymap]), o.readOnly ? EditorState.readOnly.of(true) : [], o.frame ?? [], o.extensions] }),
      parent: box.current!,
    })
    return () => view.destroy()
    // One editor per mode, as the frame keeps one per mode.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [o.mode, o.readOnly])
  return <div ref={box} data-testid="real-editor" />
}

function mount(p: Partial<EditorTabProps> = {}): void {
  const text = p.text ?? README
  render(<EditorTab filePath="/ws/quill/README.md" kind="markdown" text={text} dirty={false} saving={false} readOnly={null} cursor={{ line: 1, col: 1 }} width={1440} renderEditor={(o) => <RealEditor o={o} text={text} />} {...p} />)
}
const view = (): EditorView => EditorView.findFromDOM(document.querySelector('[data-testid="real-editor"] .cm-editor') as HTMLElement)!
const tab = (): HTMLElement => screen.getByTestId('editor-tab')
const key = (k: string, o: { shift?: boolean; alt?: boolean } = {}): void => {
  act(() => void fireEvent.keyDown(tab(), { key: k, metaKey: true, shiftKey: !!o.shift, altKey: !!o.alt }))
}
const findBar = (): HTMLElement => screen.getByRole('search', { name: 'Find in README.md' })
const count = (): string => screen.getByTestId('ed-find-count').textContent ?? ''
const typeFind = (text: string): void => void fireEvent.change(screen.getByRole('textbox', { name: 'Find' }), { target: { value: text } })

describe('find', () => {
  it('⌘F opens the bar docked under the header, with the selection as the query and the count', () => {
    mount()
    const at = README.indexOf('draft')
    act(() => view().dispatch({ selection: EditorSelection.single(at, at + 5) }))
    key('f')
    expect(findBar()).toBeTruthy()
    expect((screen.getByRole('textbox', { name: 'Find' }) as HTMLInputElement).value).toBe('draft')
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Find' }))
    expect(count()).toBe('1 of 3')
  })

  it('typing finds from the caret; Enter and Shift+Enter step and wrap; the count follows', () => {
    mount()
    key('f')
    typeFind('draft')
    expect(count()).toBe('1 of 3')
    const field = screen.getByRole('textbox', { name: 'Find' })
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(count()).toBe('2 of 3')
    fireEvent.keyDown(field, { key: 'Enter' })
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(count()).toBe('1 of 3')
    fireEvent.keyDown(field, { key: 'Enter', shiftKey: true })
    expect(count()).toBe('3 of 3')
    // ⌘G and ⇧⌘G do the same from anywhere in the tab.
    key('g')
    expect(count()).toBe('1 of 3')
    key('g', { shift: true })
    expect(count()).toBe('3 of 3')
    const sel = view().state.selection.main
    expect(view().state.sliceDoc(sel.from, sel.to)).toBe('draft')
  })

  it('Aa, W and .* are toggles; a bad pattern says so', () => {
    mount()
    key('f')
    typeFind('Quill')
    expect(count()).toBe('1 of 2')
    const bar = findBar()
    const aa = within(bar).getByRole('button', { name: 'Match case' })
    expect(aa.getAttribute('aria-pressed')).toBe('false')
    typeFind('quill')
    fireEvent.click(aa)
    expect(aa.getAttribute('aria-pressed')).toBe('true')
    expect(count()).toBe('No matches')
    fireEvent.click(aa)
    fireEvent.click(within(bar).getByRole('button', { name: 'Regular expression' }))
    typeFind('(quill')
    expect(count()).toBe('Not a valid pattern')
    fireEvent.click(within(bar).getByRole('button', { name: 'Whole words' }))
    expect(within(bar).getByRole('button', { name: 'Whole words' }).getAttribute('aria-pressed')).toBe('true')
  })

  it('no matches: Previous and Next look off but stay focusable, with the reason', () => {
    mount()
    key('f')
    typeFind('deadline')
    expect(count()).toBe('No matches')
    for (const name of ['Previous match', 'Next match']) {
      const b = within(findBar()).getByRole('button', { name })
      expect(b.getAttribute('aria-disabled')).toBe('true')
      expect(b.hasAttribute('disabled')).toBe(false)
      expect(b.title).toBe('No matches')
    }
  })

  it('Replace replaces the one you are on; Replace all is one undo step', () => {
    mount()
    key('f')
    typeFind('draft')
    fireEvent.click(within(findBar()).getByRole('button', { name: 'Show replace' }))
    fireEvent.change(screen.getByRole('textbox', { name: 'Replace with' }), { target: { value: 'sketch' } })
    fireEvent.click(within(findBar()).getByRole('button', { name: 'Replace' }))
    expect(view().state.doc.toString()).toContain('writes a sketch, then a second draft')
    expect(count()).toBe('1 of 2')
    fireEvent.click(within(findBar()).getByRole('button', { name: 'Replace all' }))
    expect(view().state.doc.toString().match(/draft/g)).toBeNull()
    expect(count()).toBe('No matches')
    expect((within(findBar()).getByRole('button', { name: 'Replace all' }) as HTMLButtonElement).getAttribute('aria-disabled')).toBe('true')
    // ⌘Z in the text: the two that Replace all changed come back, the first replace stays.
    act(() => void view().contentDOM.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, bubbles: true, cancelable: true })))
    expect(view().state.doc.toString().match(/draft/g)).toHaveLength(2)
    expect(view().state.doc.toString()).toContain('writes a sketch')
  })

  it('a read-only file can find but not replace: the toggle says why', () => {
    mount({ readOnly: 'You can view this file but not edit it' })
    key('f')
    const t = within(findBar()).getByRole('button', { name: 'Show replace' })
    expect(t.getAttribute('aria-disabled')).toBe('true')
    expect(t.title).toBe('You can view this file but not edit it')
    fireEvent.click(t)
    expect(screen.queryByRole('textbox', { name: 'Replace with' })).toBeNull()
  })

  it('Esc closes the bar and puts the keyboard back in the text; the matches go', () => {
    mount()
    key('f')
    typeFind('draft')
    expect(document.querySelectorAll('.cm-searchMatch').length).toBeGreaterThan(0)
    fireEvent.keyDown(screen.getByRole('textbox', { name: 'Find' }), { key: 'Escape' })
    expect(screen.queryByRole('search')).toBeNull()
    expect(document.querySelectorAll('.cm-searchMatch')).toHaveLength(0)
  })

  it('a match inside inline code is marked inside the code chip, never splitting it', () => {
    mount({ text: 'Intro.\n\nBriefs live in `briefs/`, one per piece.\n\nEnd.' })
    key('f')
    typeFind('brief')
    const codes = [...document.querySelectorAll('.cm-md-code')]
    expect(codes.map((c) => c.textContent)).toContain('briefs/')
    const inCode = [...document.querySelectorAll('.cm-searchMatch')].filter((m) => m.closest('.cm-md-code'))
    expect(inCode.map((m) => m.textContent)).toEqual(['brief'])
  })

  it('CodeMirror’s own search panel never shows', () => {
    mount()
    key('f')
    expect(document.querySelector('.cm-search')).toBeNull()
  })
})

describe('go to line', () => {
  it('⌘L opens it; "L9" goes to line 9 and closes it', () => {
    mount()
    key('l')
    const field = screen.getByRole('textbox', { name: 'Go to line' })
    expect(document.activeElement).toBe(field)
    fireEvent.change(field, { target: { value: 'L9' } })
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(view().state.doc.lineAt(view().state.selection.main.head).number).toBe(9)
    expect(screen.queryByRole('textbox', { name: 'Go to line' })).toBeNull()
  })

  it('out of range says how many lines there are, and stays open', () => {
    mount()
    key('l')
    const field = screen.getByRole('textbox', { name: 'Go to line' })
    fireEvent.change(field, { target: { value: '400' } })
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(screen.getByTestId('ed-goto-error').textContent).toBe('This file has 11 lines')
  })

  it('the status line’s Ln, Col is a button that opens it (code)', () => {
    mount({ filePath: '/ws/quill/src/a.ts', kind: 'code', text: 'a\nb\nc', cursor: { line: 2, col: 1 } })
    const pos = screen.getByRole('button', { name: /Ln 2, Col 1/ })
    expect(pos.title).toBe('Go to line (⌘L)')
    fireEvent.click(pos)
    expect(screen.getByRole('textbox', { name: 'Go to line' })).toBeTruthy()
  })
})

describe('the outline', () => {
  it('a markdown file has the Outline panel: its headings, indented by level', () => {
    mount()
    const toggle = screen.getByTestId('lv-panel-toggle')
    expect(toggle.getAttribute('aria-label')).toBe('Hide the outline')
    const panel = screen.getByTestId('ed-outline')
    expect(within(panel).getAllByRole('button').map((b) => b.textContent)).toEqual(['Quill', 'Setup', 'Configuration'])
    expect(within(screen.getByRole('tablist', { name: 'Side panel' })).getByRole('tab', { name: 'Outline' }).getAttribute('aria-selected')).toBe('true')
  })

  it('a click jumps to the heading', () => {
    mount()
    fireEvent.click(within(screen.getByTestId('ed-outline')).getByRole('button', { name: 'Configuration' }))
    expect(view().state.doc.lineAt(view().state.selection.main.head).number).toBe(9)
  })

  it('a code file has no outline', () => {
    mount({ filePath: '/ws/quill/src/a.ts', kind: 'code', text: 'a' })
    expect(screen.queryByTestId('ed-outline')).toBeNull()
  })
})

describe('the formatting toolbar and the / menu (Write)', () => {
  it('a selection in Write shows the toolbar; Bold wraps it', () => {
    mount()
    expect(screen.queryByRole('toolbar')).toBeNull()
    const at = README.indexOf('second')
    act(() => view().dispatch({ selection: EditorSelection.single(at, at + 6) }))
    const bar = screen.getByRole('toolbar', { name: 'Format the selection' })
    fireEvent.click(within(bar).getByRole('button', { name: 'Bold' }))
    expect(view().state.doc.toString()).toContain('a **second** draft')
  })

  it('no toolbar in Read or Source', () => {
    mount()
    key('e', { shift: true })
    const at = README.indexOf('second')
    act(() => view().dispatch({ selection: EditorSelection.single(at, at + 6) }))
    expect(screen.queryByRole('toolbar')).toBeNull()
  })

  it('"/" on an empty line opens the menu; Esc closes it', () => {
    mount()
    const at = README.indexOf('\n\n## Setup') + 1
    act(() => view().dispatch({ changes: { from: at, insert: '/' }, selection: { anchor: at + 1 }, userEvent: 'input.type' }))
    const menu = screen.getByRole('listbox', { name: 'Insert a block' })
    expect(within(menu).getAllByRole('option')[0].getAttribute('aria-selected')).toBe('true')
    act(() => void view().contentDOM.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })))
    expect(screen.queryByRole('listbox', { name: 'Insert a block' })).toBeNull()
  })

  it('a click on a block inserts it', () => {
    mount()
    const at = README.indexOf('\n\n## Setup') + 1
    act(() => view().dispatch({ changes: { from: at, insert: '/' }, selection: { anchor: at + 1 }, userEvent: 'input.type' }))
    fireEvent.mouseDown(within(screen.getByRole('listbox', { name: 'Insert a block' })).getByRole('option', { name: /Task list/ }))
    expect(view().state.doc.lineAt(at).text).toBe('- [ ] ')
  })
})
