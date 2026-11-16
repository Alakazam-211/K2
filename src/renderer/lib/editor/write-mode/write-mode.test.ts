// @vitest-environment jsdom
// Write mode (build-editors.md E2): per construct, the caret's line reveals
// its marks (muted) and every other line hides them; tasks toggle in the
// text; images resolve against the file; lists with tasks keep their
// bullets; Read is read only; nothing is revealed or hidden while an IME
// composition is open; big files open in Source.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { EditorSelection, EditorState, type Extension } from '@codemirror/state'
import { EditorView, WidgetType, type Decoration } from '@codemirror/view'
import { ensureSyntaxTree } from '@codemirror/language'
import { toggleTask, writeMode, writeModeFits, WRITE_MAX_LINES, type WriteLinks } from '.'
import { buildInline } from './inline'
import { blockField } from './blocks'
import { setComposing, THAW_MS } from './reveal'
import { BulletWidget, CheckboxWidget, FenceHeaderWidget, ImageWidget } from './widgets'
import { TableWidget } from './table'
import { resolveLinkPath } from './links'

const FILE = '/ws/quill/docs/README.md'
const views: EditorView[] = []
afterEach(() => {
  while (views.length) views.pop()!.destroy()
  document.body.innerHTML = ''
})

interface Opts {
  mode?: 'write' | 'read'
  /** Where the caret goes: the first occurrence of this text (its start), or [anchor, head]. */
  at?: string | [number, number]
  focused?: boolean
  links?: Partial<WriteLinks>
  extra?: Extension
}

function open(doc: string, o: Opts = {}): EditorView {
  const anchor = typeof o.at === 'string' ? doc.indexOf(o.at) : (o.at?.[0] ?? 0)
  const head = typeof o.at === 'string' ? anchor : (o.at?.[1] ?? anchor)
  if (anchor < 0) throw new Error(`no "${o.at}" in the doc`)
  const state = EditorState.create({
    doc,
    selection: EditorSelection.single(anchor, head),
    extensions: [writeMode(o.mode ?? 'write', { filePath: FILE, ...o.links }), o.extra ?? []],
  })
  ensureSyntaxTree(state, doc.length, 5000)
  const view = new EditorView({ state, parent: document.body })
  views.push(view)
  // jsdom can't focus a contenteditable: tell the view, as its focus event would.
  if (o.focused !== false && (o.mode ?? 'write') === 'write') focus(view)
  return view
}

function focus(view: EditorView): void {
  const eff = view.state.facet(EditorView.focusChangeEffect).map((f) => f(view.state, true)).filter((e) => e !== null)
  view.dispatch({ effects: eff })
}

interface Seen {
  from: number
  to: number
  kind: 'hide' | 'widget' | 'mark' | 'line'
  cls: string
  widget?: WidgetType
  style?: string
}

/** Every inline decoration over the whole file, as plain facts. */
function inline(view: EditorView, read = false): Seen[] {
  const { marks, hang } = buildInline(view.state, [0, view.state.doc.length], read)
  const out: Seen[] = []
  for (const set of [marks, hang]) {
    set.between(0, view.state.doc.length, (from, to, d: Decoration) => {
      const spec = d.spec as { widget?: WidgetType; class?: string; attributes?: Record<string, string> }
      if (spec.widget) out.push({ from, to, kind: 'widget', cls: '', widget: spec.widget })
      else if (from === to) out.push({ from, to, kind: 'line', cls: spec.attributes?.class ?? '', style: spec.attributes?.style })
      else if (!spec.class && !spec.attributes) out.push({ from, to, kind: 'hide', cls: '' })
      else out.push({ from, to, kind: 'mark', cls: spec.class ?? spec.attributes?.class ?? '' })
    })
  }
  return out
}

const at = (view: EditorView, text: string, nth = 0): [number, number] => {
  const doc = view.state.doc.toString()
  let i = -1
  for (let k = 0; k <= nth; k++) i = doc.indexOf(text, i + 1)
  if (i < 0) throw new Error(`no "${text}"`)
  return [i, i + text.length]
}
const isHidden = (s: Seen[], [from, to]: [number, number]): boolean => s.some((d) => d.kind === 'hide' && d.from <= from && d.to >= to)
const isMuted = (s: Seen[], [from, to]: [number, number]): boolean => s.some((d) => d.kind === 'mark' && /\bcm-md-mark\b/.test(d.cls) && d.from <= from && d.to >= to)
const markOn = (s: Seen[], cls: string, [from, to]: [number, number]): boolean => s.some((d) => d.kind === 'mark' && d.cls.split(' ').includes(cls) && d.from <= from && d.to >= to)
const lineCls = (view: EditorView, s: Seen[], lineNo: number): string => {
  const l = view.state.doc.line(lineNo)
  return s.filter((d) => d.kind === 'line' && d.from === l.from).map((d) => d.cls).join(' ')
}
const widgetsOn = (view: EditorView, s: Seen[], lineNo: number): WidgetType[] => {
  const l = view.state.doc.line(lineNo)
  return s.filter((d) => d.kind === 'widget' && d.from >= l.from && d.from <= l.to).map((d) => d.widget!)
}
const blocks = (view: EditorView): Array<{ from: number; to: number; widget: WidgetType }> => {
  const out: Array<{ from: number; to: number; widget: WidgetType }> = []
  view.state.field(blockField).decos.between(0, view.state.doc.length, (from, to, d) => {
    out.push({ from, to, widget: (d.spec as { widget: WidgetType }).widget })
  })
  return out
}

const DOC = [
  '# Harbor', // 1
  'Harbor turns interviews into **checked** articles, *fast*, with `k2 msg` and [the guide](style.md).', // 2
  '> Harbor never publishes on its own.', // 3
  '', // 4
  '- Briefs live in briefs/', // 5
  '  - Old drafts move', // 6
  '1. Clone the workspace', // 7
  '---', // 8
  'End.', // 9
].join('\n')

describe('the caret line reveals; other lines render', () => {
  it('headings: sized lines; # shows muted on the caret line, hidden elsewhere', () => {
    let v = open(DOC, { at: 'End.' })
    let s = inline(v)
    expect(lineCls(v, s, 1)).toContain('cm-md-h1')
    expect(isHidden(s, at(v, '# '))).toBe(true)
    v = open(DOC, { at: 'Harbor' })
    s = inline(v)
    expect(isMuted(s, at(v, '#'))).toBe(true)
    expect(isHidden(s, at(v, '#'))).toBe(false)
    expect(lineCls(v, s, 1)).toContain('cm-md-h1')
  })

  it('bold, italic, inline code and links: styled, their marks hidden off the caret line and muted on it', () => {
    let v = open(DOC, { at: 'End.' })
    let s = inline(v)
    expect(markOn(s, 'cm-md-strong', at(v, 'checked'))).toBe(true)
    expect(markOn(s, 'cm-md-em', at(v, 'fast'))).toBe(true)
    expect(markOn(s, 'cm-md-code', at(v, 'k2 msg'))).toBe(true)
    expect(markOn(s, 'cm-md-link', at(v, 'the guide'))).toBe(true)
    for (const m of [at(v, '**'), at(v, '**', 1), [DOC.indexOf('*fast'), DOC.indexOf('*fast') + 1] as [number, number], at(v, '`'), at(v, '['), at(v, '](style.md)')]) expect(isHidden(s, m), String(m)).toBe(true)
    v = open(DOC, { at: 'turns' })
    s = inline(v)
    expect(markOn(s, 'cm-md-strong', at(v, 'checked'))).toBe(true)
    for (const m of [at(v, '**'), at(v, '`'), at(v, '['), at(v, '](style.md)')]) {
      expect(isHidden(s, m), String(m)).toBe(false)
      expect(isMuted(s, m), String(m)).toBe(true)
    }
  })

  it('a link carries its destination, and the title says how to open it', () => {
    const v = open(DOC, { at: 'End.' })
    const link = inline(v).find((d) => d.cls === 'cm-md-link')
    expect(link).toBeTruthy()
    const dom = v.contentDOM.querySelector<HTMLElement>('.cm-md-link')!
    expect(dom.dataset.href).toBe('style.md')
    expect(dom.title).toMatch(/-click to open$/)
  })

  it('block quotes: the bar line and secondary text; > hangs muted on the caret line, hidden elsewhere', () => {
    let v = open(DOC, { at: 'End.' })
    let s = inline(v)
    expect(lineCls(v, s, 3)).toContain('cm-md-quote')
    expect(markOn(s, 'cm-md-quote-text', at(v, 'Harbor never'))).toBe(true)
    expect(isHidden(s, at(v, '> '))).toBe(true)
    v = open(DOC, { at: 'never' })
    s = inline(v)
    expect(isMuted(s, at(v, '> '))).toBe(true)
    expect(markOn(s, 'cm-md-lm', at(v, '> '))).toBe(true)
  })

  it('lists: bullets (a ring one level down) off the caret line; the marker hangs muted on it; numbers hang', () => {
    let v = open(DOC, { at: 'End.' })
    let s = inline(v)
    expect(lineCls(v, s, 5)).toContain('cm-md-li')
    expect(s.find((d) => d.kind === 'line' && d.from === v.state.doc.line(6).from && /cm-md-li/.test(d.cls))?.style).toBe('--ind: 1')
    const [b5] = widgetsOn(v, s, 5)
    const [b6] = widgetsOn(v, s, 6)
    expect(b5).toBeInstanceOf(BulletWidget)
    expect((b5 as BulletWidget).level).toBe(0)
    expect((b6 as BulletWidget).level).toBe(1)
    expect(markOn(s, 'cm-md-num', at(v, '1. '))).toBe(true)
    expect(markOn(s, 'cm-md-lm', at(v, '1. '))).toBe(true)
    v = open(DOC, { at: 'Old drafts' })
    s = inline(v)
    expect(widgetsOn(v, s, 6)).toEqual([])
    expect(isMuted(s, at(v, '  - '))).toBe(true)
    expect(markOn(s, 'cm-md-lm', at(v, '  - '))).toBe(true)
    // Its neighbour still renders.
    expect(widgetsOn(v, s, 5)[0]).toBeInstanceOf(BulletWidget)
  })

  it('horizontal rules: a hairline line, its dashes hidden; muted on the caret line', () => {
    let v = open(DOC, { at: 'End.' })
    let s = inline(v)
    expect(lineCls(v, s, 8)).toContain('cm-md-hr')
    expect(isHidden(s, at(v, '---'))).toBe(true)
    v = open(DOC, { at: '---' })
    s = inline(v)
    expect(isMuted(s, at(v, '---'))).toBe(true)
  })

  it('code fences: a card with the language and Copy in place of the fence line; its closing fence hidden', () => {
    const doc = 'Intro\n\n```ts\nconst a = 1\n```\n\n```\nplain text\n```\n'
    let v = open(doc, { at: 'Intro' })
    let s = inline(v)
    expect(lineCls(v, s, 3)).toMatch(/cm-md-fence .*cm-md-fence-open/)
    expect(lineCls(v, s, 4)).toContain('cm-md-fence')
    expect(lineCls(v, s, 5)).toMatch(/cm-md-fence-close/)
    const head = widgetsOn(v, s, 3)[0] as FenceHeaderWidget
    expect(head).toBeInstanceOf(FenceHeaderWidget)
    expect(head.label).toBe('TypeScript')
    expect(isHidden(s, [doc.indexOf('```\n\n'), doc.indexOf('```\n\n') + 3])).toBe(true)
    // A fence with no language keeps the body ink, not the inline-code ink.
    expect(markOn(s, 'cm-md-plaincode', at(v, 'plain text'))).toBe(true)
    // The Copy button copies the code, not the fences.
    const writeText = vi.fn(async () => undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    v.contentDOM.querySelector<HTMLButtonElement>('.cm-md-copy')!.click()
    expect(writeText).toHaveBeenCalledWith('const a = 1')
    // On the fence line itself, the text shows muted.
    v = open(doc, { at: '```ts' })
    s = inline(v)
    expect(widgetsOn(v, s, 3)).toEqual([])
    expect(isMuted(s, at(v, '```ts'))).toBe(true)
  })

  it('a selection across lines reveals every line it touches, and only those', () => {
    const v = open(DOC, { at: [DOC.indexOf('Harbor turns'), DOC.indexOf('never')] })
    const s = inline(v)
    expect(isHidden(s, at(v, '# '))).toBe(true)
    expect(isMuted(s, at(v, '**'))).toBe(true)
    expect(isMuted(s, at(v, '> '))).toBe(true)
    expect(widgetsOn(v, s, 5)[0]).toBeInstanceOf(BulletWidget)
  })

  it('blank lines are short, except the one the caret is on; a selection over one leaves it short', () => {
    const doc = 'One\n\nTwo\n\nThree'
    let v = open(doc, { at: 'Three' })
    expect(lineCls(v, inline(v), 2)).toContain('cm-md-blank')
    v = open(doc, { at: [doc.indexOf('Two') - 1, doc.indexOf('Two') - 1] })
    expect(lineCls(v, inline(v), 2)).not.toContain('cm-md-blank')
    v = open(doc, { at: [0, doc.indexOf('Three')] })
    expect(lineCls(v, inline(v), 2)).toContain('cm-md-blank')
    expect(lineCls(v, inline(v), 4)).toContain('cm-md-blank')
  })

  it('an unfocused editor reveals nothing', () => {
    const v = open(DOC, { at: 'turns', focused: false })
    const s = inline(v)
    expect(isHidden(s, at(v, '**'))).toBe(true)
  })
})

describe('tasks', () => {
  const TASKS = '- plain item\n- [ ] Add the checker\n- [x] Wire the outline\n- another item\n\nEnd.'

  it('a list with tasks keeps its bullets: plain items get bullets, tasks get checkboxes (audit P5)', () => {
    const v = open(TASKS, { at: 'End.' })
    const s = inline(v)
    expect(widgetsOn(v, s, 1)[0]).toBeInstanceOf(BulletWidget)
    expect(widgetsOn(v, s, 2)[0]).toBeInstanceOf(CheckboxWidget)
    expect((widgetsOn(v, s, 2)[0] as CheckboxWidget).checked).toBe(false)
    expect((widgetsOn(v, s, 3)[0] as CheckboxWidget).checked).toBe(true)
    expect(widgetsOn(v, s, 4)[0]).toBeInstanceOf(BulletWidget)
    expect(lineCls(v, s, 2)).toContain('cm-md-task')
    expect(markOn(s, 'cm-md-done', at(v, 'Wire the outline'))).toBe(true)
    // The DOM has the bullets and K2's checkboxes, cut in Terminal (chip chamfer).
    expect(v.contentDOM.querySelectorAll('.cm-md-bullet')).toHaveLength(2)
    const boxes = v.contentDOM.querySelectorAll<HTMLInputElement>('input.k2-checkbox[type="checkbox"]')
    expect(boxes).toHaveLength(2)
    expect(boxes[0].dataset.k2Chamfer).toBe('chip')
  })

  it('on the caret line the task marker hangs muted, so the text stays put', () => {
    const v = open(TASKS, { at: 'Add the' })
    const s = inline(v)
    expect(widgetsOn(v, s, 2)).toEqual([])
    expect(markOn(s, 'cm-md-lm', at(v, '- [ ] '))).toBe(true)
    expect(isMuted(s, at(v, '- [ ] '))).toBe(true)
  })

  it('a click toggles [ ] and [x] in the text, as one undoable edit, and the caret stays', () => {
    const v = open(TASKS, { at: 'End.' })
    const caret = v.state.selection.main.head
    const box = (): HTMLInputElement => v.contentDOM.querySelectorAll<HTMLInputElement>('input.k2-checkbox')[0]
    box().click()
    expect(v.state.doc.line(2).text).toBe('- [x] Add the checker')
    expect(v.state.selection.main.head).toBe(caret)
    box().click()
    expect(v.state.doc.line(2).text).toBe('- [ ] Add the checker')
  })

  it('Space on a focused checkbox toggles it, and the box keeps focus', () => {
    const v = open(TASKS, { at: 'End.' })
    const box = v.contentDOM.querySelectorAll<HTMLInputElement>('input.k2-checkbox')[1]
    box.focus()
    box.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true }))
    expect(v.state.doc.line(3).text).toBe('- [ ] Wire the outline')
    const again = v.contentDOM.querySelectorAll<HTMLInputElement>('input.k2-checkbox')[1]
    expect(again).toBe(box)
    expect(again.checked).toBe(false)
  })

  it('in Read a checkbox shows its state but never changes the text', () => {
    const v = open(TASKS, { mode: 'read' })
    const box = v.contentDOM.querySelectorAll<HTMLInputElement>('input.k2-checkbox')[0]
    expect(box.getAttribute('aria-disabled')).toBe('true')
    box.click()
    box.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true }))
    expect(v.state.doc.toString()).toBe(TASKS)
  })
})

describe('tables', () => {
  const T = 'Intro\n\n| Setting | Default |\n|---|--:|\n| `tone` | **plain** |\n| maxWords | 1800 |\n\nEnd.'

  it('render as a table while the caret is outside, with the cells inline formatting and alignment', () => {
    const v = open(T, { at: 'End.' })
    const [t] = blocks(v).filter((b) => b.widget instanceof TableWidget)
    expect(t).toBeTruthy()
    expect([t.from, t.to]).toEqual([T.indexOf('| Setting'), T.indexOf('1800 |') + 6])
    const table = v.dom.querySelector('table.cm-md-table')!
    expect([...table.querySelectorAll('th')].map((c) => c.textContent)).toEqual(['Setting', 'Default'])
    expect(table.querySelector('td .cm-md-code')?.textContent).toBe('tone')
    expect(table.querySelector('td .cm-md-strong')?.textContent).toBe('plain')
    expect((table.querySelectorAll('td')[1] as HTMLElement).style.textAlign).toBe('right')
  })

  it('show their text, every line, when the caret is inside', () => {
    const v = open(T, { at: 'maxWords' })
    expect(blocks(v).filter((b) => b.widget instanceof TableWidget)).toEqual([])
    const s = inline(v)
    for (const n of [3, 4, 5, 6]) expect(lineCls(v, s, n)).toContain('cm-md-table-src')
    expect(isMuted(s, at(v, '**'))).toBe(true)
  })

  it('a click in a cell puts the caret in that cell, which reveals the table', () => {
    const v = open(T, { at: 'End.' })
    const cell = v.dom.querySelectorAll<HTMLElement>('table.cm-md-table td')[2]
    cell.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0 }))
    expect(v.state.selection.main.head).toBe(T.indexOf('maxWords'))
    expect(blocks(v).filter((b) => b.widget instanceof TableWidget)).toEqual([])
  })

  it('the block set is mapped through edits elsewhere, not rebuilt; a fence opened above swallows the table', () => {
    const v = open(T, { at: 'End.' })
    const before = blocks(v).find((b) => b.widget instanceof TableWidget)!.widget
    v.dispatch({ changes: { from: 0, insert: 'More ' }, selection: { anchor: 5 }, userEvent: 'input.type' })
    const after = blocks(v).find((b) => b.widget instanceof TableWidget)
    expect(after?.widget).toBe(before)
    expect(after?.from).toBe(T.indexOf('| Setting') + 5)
    v.dispatch({ changes: { from: 0, insert: '```\n' }, selection: { anchor: T.length + 9 } })
    ensureSyntaxTree(v.state, v.state.doc.length, 5000)
    v.dispatch({})
    expect(blocks(v).filter((b) => b.widget instanceof TableWidget)).toEqual([])
  })
})

describe('images', () => {
  const IMG = 'Intro\n\n![The outline view](img/outline.png)\n\n![Up](../shared/logo.png)\n\n![Web](https://example.com/a.png)\n\nEnd.'

  it('resolve against the file\'s folder and load through the host; the line under the picture is its caption', async () => {
    const loadImage = vi.fn(async (p: string) => `blob:${p}`)
    const v = open(IMG, { at: 'End.', links: { loadImage } })
    const imgs = blocks(v).filter((b) => b.widget instanceof ImageWidget)
    expect(imgs.map((b) => [(b.widget as ImageWidget).src, (b.widget as ImageWidget).alt])).toEqual([
      ['img/outline.png', 'The outline view'],
      ['../shared/logo.png', 'Up'],
      ['https://example.com/a.png', 'Web'],
    ])
    // Each sits above its own line.
    expect(imgs[0].from).toBe(IMG.indexOf('![The'))
    expect(loadImage.mock.calls.map((c) => c[0])).toEqual(['/ws/quill/docs/img/outline.png', '/ws/quill/shared/logo.png'])
    await Promise.resolve()
    await Promise.resolve()
    // jsdom draws only the first part of the file (its viewport), which holds the two local pictures.
    const srcs = [...v.dom.querySelectorAll<HTMLImageElement>('.cm-md-figure img')].map((i) => i.getAttribute('src'))
    expect(srcs.slice(0, 2)).toEqual(['blob:/ws/quill/docs/img/outline.png', 'blob:/ws/quill/shared/logo.png'])
    // A web picture loads as it is.
    expect(new ImageWidget('https://example.com/a.png', 'Web').toDOM(v).querySelector('img')!.getAttribute('src')).toBe('https://example.com/a.png')
    const s = inline(v)
    expect(markOn(s, 'cm-md-caption', at(v, 'The outline view'))).toBe(true)
    expect(isHidden(s, at(v, '!['))).toBe(true)
    expect(isHidden(s, at(v, '](img/outline.png)'))).toBe(true)
  })

  it('a picture that can\'t load says so, with its path', async () => {
    const v = open('![Gone](missing.png)\n\nEnd.', { at: 'End.', links: { loadImage: async () => Promise.reject(new Error('not found')) } })
    await new Promise((r) => setTimeout(r, 0))
    const fig = v.dom.querySelector<HTMLElement>('.cm-md-figure')!
    expect(fig.hasAttribute('data-missing')).toBe(true)
    expect(fig.textContent).toBe("Can't show missing.png")
  })

  it('paths: relative, parent, absolute, encoded, anchors and Windows', () => {
    expect(resolveLinkPath('/ws/a/README.md', 'docs/x.png')).toBe('/ws/a/docs/x.png')
    expect(resolveLinkPath('/ws/a/README.md', './docs/../y.md#top')).toBe('/ws/a/y.md')
    expect(resolveLinkPath('/ws/a/README.md', '/etc/z.png')).toBe('/etc/z.png')
    expect(resolveLinkPath('/ws/a/README.md', 'my%20file.png')).toBe('/ws/a/my file.png')
    expect(resolveLinkPath('/ws/a/README.md', '#section')).toBeNull()
    expect(resolveLinkPath('/ws/a/README.md', 'vscode://x')).toBeNull()
    expect(resolveLinkPath('C:\\ws\\a\\README.md', 'img/o.png')).toBe('C:\\ws\\a\\img\\o.png')
  })
})

describe('links', () => {
  it('Write: ⌘-click (Ctrl-click) opens; a plain click only places the caret', () => {
    const openFile = vi.fn()
    const openExternal = vi.fn()
    const v = open('See [the guide](style.md) and https://example.com/x.\n\nEnd.', { at: 'End.', links: { openFile, openExternal } })
    const [guide, web] = [...v.contentDOM.querySelectorAll<HTMLElement>('.cm-md-link')]
    guide.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0 }))
    expect(openFile).not.toHaveBeenCalled()
    guide.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0, metaKey: true }))
    expect(openFile).toHaveBeenCalledWith('/ws/quill/docs/style.md')
    web.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0, ctrlKey: true }))
    expect(openExternal).toHaveBeenCalledWith('https://example.com/x')
  })

  it('Read: a plain click opens', () => {
    const openFile = vi.fn()
    const v = open('See [the guide](style.md).', { mode: 'read', links: { openFile } })
    v.contentDOM.querySelector<HTMLElement>('.cm-md-link')!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, button: 0 }))
    expect(openFile).toHaveBeenCalledWith('/ws/quill/docs/style.md')
  })
})

describe('Read', () => {
  it('is the same rendered page, read only, with no caret and no reveal', () => {
    const v = open(DOC, { mode: 'read', at: 'turns' })
    expect(v.state.readOnly).toBe(true)
    expect(v.contentDOM.getAttribute('contenteditable')).toBe('false')
    expect(v.contentDOM.getAttribute('aria-readonly')).toBe('true')
    expect(v.dom.classList.contains('cm-md-read')).toBe(true)
    // The caret's line in Write would reveal; in Read it renders.
    focus(v)
    const s = inline(v, true)
    expect(isHidden(s, at(v, '**'))).toBe(true)
    expect(isHidden(s, at(v, '# '))).toBe(true)
    expect(widgetsOn(v, s, 5)[0]).toBeInstanceOf(BulletWidget)
    // Commands that edit refuse (the keys can still select and scroll).
    expect(toggleTask(v, 0)).toBe(false)
  })
})

describe('IME composition', () => {
  it('nothing is revealed or hidden while a composition is open; the reveal follows the caret after it ends', async () => {
    const v = open(DOC, { at: 'turns' })
    const lineOf = (text: string): number => v.state.doc.lineAt(DOC.indexOf(text)).number
    const revealedLine = (): number[] => {
      const s = inline(v)
      return [lineOf('turns'), lineOf('never')].filter((n) => isMuted(s, n === lineOf('turns') ? at(v, '**') : at(v, '> ')))
    }
    expect(revealedLine()).toEqual([lineOf('turns')])
    v.contentDOM.dispatchEvent(new CompositionEvent('compositionstart', { data: '' }))
    // The IME moves the caret and composes text on another line: the reveal holds.
    v.dispatch({ selection: { anchor: DOC.indexOf('never') } })
    v.dispatch({ changes: { from: DOC.indexOf('never'), insert: 'に' }, userEvent: 'input.type.compose' })
    expect(revealedLine()).toEqual([lineOf('turns')])
    expect(isHidden(inline(v), at(v, '> '))).toBe(true)
    v.contentDOM.dispatchEvent(new CompositionEvent('compositionend', { data: 'に' }))
    // Still frozen until CodeMirror has flushed the composed text.
    expect(revealedLine()).toEqual([lineOf('turns')])
    await new Promise((r) => setTimeout(r, THAW_MS + 40))
    expect(revealedLine()).toEqual([lineOf('never')])
  })

  it('a composed keystroke freezes the reveal even before compositionstart arrives', () => {
    const v = open(DOC, { at: 'turns' })
    v.dispatch({ changes: { from: DOC.indexOf('never'), insert: 'に' }, selection: { anchor: DOC.indexOf('never') + 1 }, userEvent: 'input.type.compose' })
    expect(isMuted(inline(v), at(v, '**'))).toBe(true)
    v.dispatch({ effects: setComposing.of(false) })
    expect(isMuted(inline(v), at(v, '**'))).toBe(false)
  })
})

describe('large files open in Source', () => {
  it('over 20,000 lines or 1 MB, Write and Read are off', () => {
    expect(writeModeFits('# a\n'.repeat(100))).toBe(true)
    expect(writeModeFits('a\n'.repeat(WRITE_MAX_LINES - 1))).toBe(true)
    expect(writeModeFits('a\n'.repeat(WRITE_MAX_LINES))).toBe(false)
    expect(writeModeFits('é'.repeat(600_000))).toBe(false) // 1.2 MB in UTF-8
    expect(writeModeFits('a'.repeat(1_000_000))).toBe(true)
  })
})
