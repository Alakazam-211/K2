// @vitest-environment jsdom
// The editor frame on the live shell (build-editors.md E1b): one slim header
// (the file once, Write | Read | Source for markdown, nothing else), the mode
// keys, read-only modes, the status line for markdown and code, Display at
// 800, and Read rendering the buffer.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { useSettingsStore } from '@/stores/settings'
import { EditorTab, type EditorTabProps, type EditorMountOptions } from './EditorTab'

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const README = '# Quill\n\nQuill drafts, edits and fact-checks writing.\n\n- [ ] Add the checker\n'
const TS = "import { x } from './x'\r\nexport const y = x + 1\r\n"

/** The editor CodeMirror would be: its mount options as attributes. */
function stubEditor(o: EditorMountOptions): React.ReactNode {
  const ext = Array.isArray(o.extensions) && o.extensions.length === 0 ? 'none' : 'write-mode'
  return <div data-testid="stub-editor" data-mode={o.mode} data-ext={ext} data-read-only={String(o.readOnly)} data-line-numbers={String(o.lineNumbers)} data-wrap={String(o.wrap)} />
}

function mount(p: Partial<EditorTabProps> = {}): ReturnType<typeof render> {
  return render(
    <EditorTab
      filePath="/ws/quill/README.md"
      kind="markdown"
      text={README}
      dirty={false}
      saving={false}
      readOnly={null}
      cursor={{ line: 3, col: 7 }}
      width={1440}
      renderEditor={stubEditor}
      {...p}
    />,
  )
}

const modes = (): HTMLElement => screen.getByRole('radiogroup', { name: 'Mode' })
const mode = (): string | null => screen.getByTestId('editor-tab').getAttribute('data-mode')
const cmdE = (shift = false): void => {
  fireEvent.keyDown(screen.getByTestId('editor-tab'), { key: shift ? 'E' : 'e', metaKey: true, shiftKey: shift })
}

describe('header', () => {
  it('markdown: the file once (path as its tooltip), the tile, Write | Read | Source, and nothing else', () => {
    mount()
    const head = screen.getByTestId('lv-shell-header')
    const name = within(head).getByText('README.md')
    expect(name.getAttribute('title')).toBe('/ws/quill/README.md')
    expect(within(head).getAllByText('README.md')).toHaveLength(1)
    expect(head.querySelector('[data-k2-chamfer="control"].lv-file-glyph')).toBeTruthy()
    expect(within(modes()).getAllByRole('radio').map((r) => r.textContent)).toEqual(['Write', 'Read', 'Source'])
    // No state chip on a plain file, no old toolbar, no path bar; the panel toggle is the Outline's (E3).
    expect(screen.queryByTestId('lv-state')).toBeNull()
    expect(screen.getByTestId('lv-panel-toggle').getAttribute('aria-label')).toBe('Hide the outline')
    for (const t of ['Search (Cmd+F)', 'Toggle word wrap', 'Refresh', 'Close', 'Pin tab (keeps it at the left)']) expect(document.querySelector(`[title="${t}"]`)).toBeNull()
    expect(screen.getByTestId('editor-tab').textContent).not.toContain('/ws/quill/README.md')
  })

  it('code: no mode control, no outline', () => {
    mount({ filePath: '/ws/quill/src/brief.ts', kind: 'code', text: TS })
    expect(screen.queryByRole('radiogroup', { name: 'Mode' })).toBeNull()
    expect(screen.queryByTestId('lv-panel-toggle')).toBeNull()
    expect(screen.getByTestId('stub-editor').getAttribute('data-line-numbers')).toBe('true')
  })
})

describe('modes and keys', () => {
  it('opens in Write; ⌘E toggles Write and Read; ⇧⌘E opens Source; clicks choose', () => {
    mount()
    expect(mode()).toBe('write')
    expect(within(modes()).getByRole('radio', { name: 'Write' }).getAttribute('aria-checked')).toBe('true')
    // Write and Read are the rendered page without line numbers; Source has them.
    expect(screen.getByTestId('stub-editor').getAttribute('data-line-numbers')).toBe('false')
    cmdE()
    expect(mode()).toBe('read')
    expect(screen.getByTestId('stub-editor').getAttribute('data-line-numbers')).toBe('false')
    cmdE()
    expect(mode()).toBe('write')
    cmdE(true)
    expect(mode()).toBe('source')
    expect(screen.getByTestId('stub-editor').getAttribute('data-line-numbers')).toBe('true')
    cmdE()
    expect(mode()).toBe('write')
    fireEvent.click(within(modes()).getByRole('radio', { name: 'Read' }))
    expect(mode()).toBe('read')
  })

  it('a read-only file opens in Read; Write is off but focusable, with its reason on hover and on focus; Source stays', () => {
    mount({ readOnly: 'You can view this file but not edit it' })
    expect(mode()).toBe('read')
    const write = within(modes()).getByRole('radio', { name: /Write/ })
    expect(write.getAttribute('aria-disabled')).toBe('true')
    expect(write.hasAttribute('disabled')).toBe(false)
    const tip = screen.getByRole('tooltip', { hidden: true })
    expect(tip.textContent).toBe('You can view this file but not edit it')
    expect(write.getAttribute('aria-describedby')).toBe(tip.id)
    expect(tip.hidden).toBe(true)
    act(() => write.focus())
    expect(document.activeElement).toBe(write)
    expect(tip.hidden).toBe(false)
    act(() => write.blur())
    expect(tip.hidden).toBe(true)
    fireEvent.mouseEnter(write)
    expect(tip.hidden).toBe(false)
    fireEvent.mouseLeave(write)
    expect(tip.hidden).toBe(true)
    // Clicking it or ⌘E never reaches Write.
    fireEvent.click(write)
    expect(mode()).toBe('read')
    cmdE()
    expect(mode()).toBe('read')
    // Source is available, and read only.
    fireEvent.click(within(modes()).getByRole('radio', { name: 'Source' }))
    expect(mode()).toBe('source')
    expect(screen.getByTestId('stub-editor').getAttribute('data-read-only')).toBe('true')
    cmdE()
    expect(mode()).toBe('read')
  })

  it('the arrow keys move through Write even while it is off', () => {
    mount({ readOnly: 'You can view this file but not edit it' })
    const read = within(modes()).getByRole('radio', { name: 'Read' })
    act(() => read.focus())
    fireEvent.keyDown(read, { key: 'ArrowLeft' })
    expect(document.activeElement).toBe(within(modes()).getByRole('radio', { name: /Write/ }))
    expect(mode()).toBe('read')
  })
})

describe('status line', () => {
  const stat = (id: string): string | null => screen.queryByTestId(`ed-stat-${id}`)?.textContent ?? null
  const toggles = (): string[] => within(screen.getByTestId('editor-status')).queryAllByRole('button').filter((b) => b.hasAttribute('aria-pressed')).map((b) => b.textContent ?? '')

  it('markdown: words, reading time and the save word; Full width and Wrap', () => {
    mount()
    expect(stat('words')).toBe('10 words')
    expect(stat('read')).toBe('1 min read')
    expect(stat('save')).toBe('Saved')
    expect(stat('pos')).toBeNull()
    expect(toggles()).toEqual(['Full width', 'Wrap'])
  })

  it('the save word says the truth: Not saved while dirty, Saving while it saves', () => {
    const r = mount({ dirty: true })
    expect(stat('save')).toBe('Not saved')
    r.rerender(<EditorTab filePath="/ws/quill/README.md" kind="markdown" text={README} dirty saving readOnly={null} cursor={{ line: 1, col: 1 }} width={1440} renderEditor={stubEditor} />)
    expect(stat('save')).toBe('Saving')
  })

  it('Read shows Full width only; Source shows Wrap and Ln, Col', () => {
    mount()
    cmdE()
    expect(toggles()).toEqual(['Full width'])
    cmdE(true)
    expect(toggles()).toEqual(['Wrap'])
    expect(stat('pos')).toBe('Ln 3, Col 7')
  })

  it('Full width and Wrap toggle and stay on this computer', () => {
    mount()
    const full = screen.getByRole('button', { name: 'Full width' })
    expect(full.getAttribute('aria-pressed')).toBe('false')
    fireEvent.click(full)
    expect(full.getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByTestId('editor-tab').hasAttribute('data-full')).toBe(true)
    const wrap = screen.getByRole('button', { name: 'Wrap' })
    expect(wrap.getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(wrap)
    expect(screen.getByTestId('stub-editor').getAttribute('data-wrap')).toBe('false')
    cleanup()
    mount()
    expect(screen.getByRole('button', { name: 'Full width' }).getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByRole('button', { name: 'Wrap' }).getAttribute('aria-pressed')).toBe('false')
  })

  it('a read-only file has no save word', () => {
    mount({ readOnly: 'You can view this file but not edit it' })
    expect(stat('save')).toBeNull()
    expect(stat('words')).toBe('10 words')
  })

  it('code: Ln, Col and the save word; Wrap; language, encoding and line endings', () => {
    useSettingsStore.setState((s) => ({ editor: { ...s.editor, wordWrap: false } }))
    mount({ filePath: '/ws/quill/src/brief.ts', kind: 'code', text: TS, cursor: { line: 2, col: 14 } })
    expect(stat('pos')).toBe('Ln 2, Col 14')
    expect(stat('save')).toBe('Saved')
    expect(stat('words')).toBeNull()
    expect(stat('lang')).toBe('TypeScript')
    expect(stat('enc')).toBe('UTF-8')
    expect(stat('eol')).toBe('CRLF')
    expect(toggles()).toEqual(['Wrap'])
    const wrap = screen.getByRole('button', { name: 'Wrap' })
    expect(wrap.getAttribute('aria-pressed')).toBe('false')
    fireEvent.click(wrap)
    expect(useSettingsStore.getState().editor.wordWrap).toBe(true)
  })

  it('at 800 the toggles sit behind Display, which opens them in a group', () => {
    mount({ width: 784 })
    expect(screen.queryByRole('button', { name: 'Full width' })).toBeNull()
    const display = screen.getByRole('button', { name: 'Display' })
    expect(display.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(display)
    expect(display.getAttribute('aria-expanded')).toBe('true')
    const group = screen.getByRole('group', { name: 'Display' })
    expect(within(group).getAllByRole('button').map((b) => b.textContent)).toEqual(['Full width', 'Wrap'])
    fireEvent.keyDown(group, { key: 'Escape' })
    expect(screen.queryByRole('group', { name: 'Display' })).toBeNull()
    expect(stat('words')).toBe('10 words')
  })

  it('code at 800: Wrap and the file facts go behind Display', () => {
    mount({ filePath: '/ws/quill/src/brief.ts', kind: 'code', text: TS, width: 784 })
    expect(stat('lang')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Display' }))
    const group = screen.getByRole('group', { name: 'Display' })
    expect(within(group).getByRole('button', { name: 'Wrap' })).toBeTruthy()
    expect(group.textContent).toContain('TypeScript')
    expect(stat('pos')).toBe('Ln 3, Col 7')
  })
})

describe('Write, Read and Source (E2)', () => {
  it('Write and Read mount the editor with the rendered page; Source with nothing; the same editor element throughout', () => {
    mount()
    const ed = screen.getByTestId('stub-editor')
    expect([ed.dataset.mode, ed.dataset.ext]).toEqual(['write', 'write-mode'])
    cmdE()
    expect(screen.getByTestId('stub-editor')).toBe(ed)
    expect([ed.dataset.mode, ed.dataset.ext, ed.dataset.wrap]).toEqual(['read', 'write-mode', 'true'])
    expect(screen.getByTestId('editor-read').contains(ed)).toBe(true)
    cmdE(true)
    expect(screen.getByTestId('stub-editor')).toBe(ed)
    expect([ed.dataset.mode, ed.dataset.ext]).toEqual(['source', 'none'])
    // Read no longer uses today's renderer.
    expect(document.querySelector('.markdown-content')).toBeNull()
  })

  it('a code file has no Write mode extension', () => {
    mount({ filePath: '/ws/quill/src/x.ts', kind: 'code', text: TS })
    expect(screen.getByTestId('stub-editor').dataset.ext).toBe('none')
  })

  it('a file over 20,000 lines opens in Source; Write and Read are off with the reason; ⌘E stays in Source', () => {
    mount({ text: '- item\n'.repeat(20_001) })
    expect(mode()).toBe('source')
    expect(screen.getByTestId('stub-editor').dataset.ext).toBe('none')
    for (const name of ['Write', 'Read']) {
      const seg = within(modes()).getByRole('radio', { name: new RegExp(name) })
      expect(seg.getAttribute('aria-disabled')).toBe('true')
    }
    expect(within(modes()).getByRole('radio', { name: 'Source' }).getAttribute('aria-disabled')).toBeNull()
    cmdE()
    expect(mode()).toBe('source')
    fireEvent.click(within(modes()).getByRole('radio', { name: /Write/ }))
    expect(mode()).toBe('source')
  })
})

describe('Read', () => {
  it('⌘S in Read saves the buffer', () => {
    let saved = 0
    mount({ dirty: true, onSave: () => saved++ })
    cmdE()
    fireEvent.keyDown(screen.getByTestId('editor-tab'), { key: 's', metaKey: true })
    expect(saved).toBe(1)
  })
})
